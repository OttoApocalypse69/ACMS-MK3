//! Commands for inks, patterns, symmetry, tiled drawing, custom brushes, the palette and
//! reference layers.

use std::path::{Path, PathBuf};

use acms_core::color::Rgba8;
use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::edit;
use acms_core::palette::{self, MAX_RAMP_STEPS, SortKey};
use acms_core::pattern::{MAX_DENSITY, Pattern};
use acms_core::sprite::{
    LayerId, MAX_RADIAL, MAX_SPRITE_SIZE, Palette, Slot, Sprite, SpriteError, Symmetry,
};
use acms_core::tool::InkKind;
use acms_io::palette::{load_palette, save_palette};
use acms_io::png::decode_png;

use crate::commands::sentence;
use crate::layer_commands::transact;
use crate::state::{AppState, Dialog, FileRequest};

type CommandResult = Result<(), CommandError>;

/// Steps offered by the Generate Ramp dialog.
const DEFAULT_RAMP_STEPS: u32 = 5;

fn has_document(state: &AppState) -> bool {
    state.editor.document().is_some()
}

fn has_colors_selected(state: &AppState) -> bool {
    has_document(state) && !state.swatch_selection.is_empty()
}

fn failed(err: SpriteError) -> CommandError {
    CommandError::failed(sentence(err))
}

fn sprite(state: &AppState) -> Result<&Sprite, CommandError> {
    state
        .editor
        .document()
        .map(|document| document.sprite())
        .ok_or_else(|| CommandError::failed("No sprite is open."))
}

/// The selected layer and frame of the open sprite.
fn target(state: &AppState) -> Result<(LayerId, usize), CommandError> {
    let document = state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))?;
    Ok((document.active_layer(), document.active_frame()))
}

/// Replaces the sprite's palette as one undo step.
fn set_palette(state: &mut AppState, name: &str, palette: Palette) -> CommandResult {
    transact(state, name, |tx| {
        tx.set_palette(palette);
        Ok(())
    })
}

/// The palette positions a command is about: the `indices` argument (`"0,3,4"`) if given,
/// otherwise the colours selected in the Palette panel, in the order they were picked.
fn indices(state: &AppState, params: &Params) -> Result<Vec<usize>, CommandError> {
    let Some(text) = params.text("indices") else {
        return Ok(state.swatch_selection.clone());
    };
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse().map_err(|_| {
                CommandError::failed(format!("\"{part}\" is not a position in the palette."))
            })
        })
        .collect()
}

fn set_ink(state: &mut AppState, params: &Params) -> CommandResult {
    let id = params.text("ink").unwrap_or_default();
    let ink = InkKind::from_id(id).ok_or_else(|| {
        let known: Vec<&str> = InkKind::ALL.iter().map(|ink| ink.id()).collect();
        CommandError::failed(format!(
            "\"{id}\" is not an ink. The inks are: {}.",
            known.join(", ")
        ))
    })?;
    state.editor.options.ink = ink;
    Ok(())
}

fn set_pattern(state: &mut AppState, params: &Params) -> CommandResult {
    let id = params.text("pattern").unwrap_or_default();
    let pattern = if id == "custom" {
        // The one made last with "Dither Pattern from Selection".
        state.editor.custom_pattern.clone().ok_or_else(|| {
            CommandError::failed("There is no custom pattern yet: make one from a selection.")
        })?
    } else {
        Pattern::built_in(id).ok_or_else(|| {
            let known: Vec<&str> = Pattern::BUILT_IN.iter().map(Pattern::id).collect();
            CommandError::failed(format!(
                "\"{id}\" is not a pattern. The patterns are: {}, custom.",
                known.join(", ")
            ))
        })?
    };
    state.editor.options.pattern = pattern;
    Ok(())
}

fn pattern_from_selection(state: &mut AppState, _: &Params) -> CommandResult {
    let (layer, frame) = target(state)?;
    let pattern = edit::pattern_from_selection(sprite(state)?, layer, frame).map_err(failed)?;
    state.editor.custom_pattern = Some(pattern.clone());
    state.editor.options.pattern = pattern;
    state.editor.options.ink = InkKind::Dither;
    Ok(())
}

fn brush_from_selection(state: &mut AppState, _: &Params) -> CommandResult {
    let (layer, frame) = target(state)?;
    state.editor.brush =
        edit::brush_from_selection(sprite(state)?, layer, frame).map_err(failed)?;
    Ok(())
}

fn set_symmetry(state: &mut AppState, symmetry: Symmetry) -> CommandResult {
    if let Symmetry::Radial(count) = symmetry {
        state.radial_count = count;
    }
    let document = state
        .editor
        .document_mut()
        .ok_or_else(|| CommandError::failed("No sprite is open."))?;
    document.set_symmetry(symmetry);
    Ok(())
}

/// The `count` argument for a radial symmetry, or the count used last.
fn radial_count(state: &AppState, params: &Params) -> u8 {
    match params.int("count") {
        Some(count) => count.clamp(2, i64::from(MAX_RADIAL)) as u8,
        None => state.radial_count,
    }
}

fn symmetry_set(state: &mut AppState, params: &Params) -> CommandResult {
    let id = params.text("mode").unwrap_or_default();
    let symmetry = Symmetry::from_id(id, radial_count(state, params)).ok_or_else(|| {
        CommandError::failed(format!(
            "\"{id}\" is not a symmetry. They are: off, horizontal, vertical, both, radial."
        ))
    })?;
    set_symmetry(state, symmetry)
}

fn palette_import(state: &mut AppState, params: &Params) -> CommandResult {
    let Some(path) = params.text("path").map(PathBuf::from) else {
        state.file_request = Some(FileRequest::ImportPalette);
        return Ok(());
    };
    let loaded = load_palette(&path)
        .map_err(|err| CommandError::failed(format!("Could not read {}: {err}", path.display())))?;
    let count = loaded.palette.len();
    set_palette(state, "Import Palette", loaded.palette)?;
    state.swatch_selection.clear();
    let note = if loaded.truncated {
        " (the picture has more; these are the first it shows)"
    } else {
        ""
    };
    state.log.push(format!(
        "Imported {count} colours from {}{note}",
        path.display()
    ));
    Ok(())
}

fn palette_export(state: &mut AppState, params: &Params) -> CommandResult {
    let Some(path) = params.text("path") else {
        state.file_request = Some(FileRequest::ExportPalette);
        return Ok(());
    };
    let path = Path::new(path);
    save_palette(sprite(state)?.palette(), path).map_err(|err| {
        CommandError::failed(format!("Could not write {}: {err}", path.display()))
    })?;
    state.log.push(format!("Exported {}", path.display()));
    Ok(())
}

fn palette_sort(state: &mut AppState, name: &str, key: SortKey) -> CommandResult {
    let sorted = palette::sorted(sprite(state)?.palette(), key);
    set_palette(state, name, sorted)?;
    // The positions the selection was about now hold other colours.
    state.swatch_selection.clear();
    Ok(())
}

fn palette_add(state: &mut AppState, params: &Params) -> CommandResult {
    let color = params.color("color").unwrap_or(state.editor.primary);
    if color.a == 0 {
        return Err(CommandError::failed(
            "A fully transparent colour cannot go in the palette.",
        ));
    }
    let with = palette::with_color(sprite(state)?.palette(), color);
    set_palette(state, "Add Colour", with)
}

fn palette_remove(state: &mut AppState, params: &Params) -> CommandResult {
    let indices = indices(state, params)?;
    if indices.is_empty() {
        return Err(CommandError::failed("Select the colours to remove first."));
    }
    let without = palette::without_entries(sprite(state)?.palette(), &indices);
    set_palette(state, "Remove Colours", without)?;
    state.swatch_selection.clear();
    Ok(())
}

fn palette_new_ramp(state: &mut AppState, params: &Params) -> CommandResult {
    let indices = indices(state, params)?;
    let palette = sprite(state)?.palette();
    let colors: Vec<Rgba8> = indices
        .iter()
        .filter_map(|&index| palette.entries().get(index))
        .map(|entry| entry.color)
        .collect();
    let with = palette::with_ramp(palette, &colors).ok_or_else(|| {
        CommandError::failed("A ramp needs at least two different colours: select them first.")
    })?;
    set_palette(state, "New Ramp", with)
}

fn palette_delete_ramp(state: &mut AppState, params: &Params) -> CommandResult {
    let palette = sprite(state)?.palette();
    let index = usize::try_from(params.int("ramp").unwrap_or(-1)).unwrap_or(usize::MAX);
    if index >= palette.ramps().len() {
        return Err(CommandError::failed("There is no such ramp."));
    }
    let without = palette::without_ramp(palette, index);
    set_palette(state, "Delete Ramp", without)
}

fn palette_generate_ramp(state: &mut AppState, params: &Params) -> CommandResult {
    let palette = sprite(state)?.palette();
    let Some(steps) = params.int("steps") else {
        state.dialog = Some(Dialog::Ramp {
            steps: DEFAULT_RAMP_STEPS,
        });
        return Ok(());
    };
    let steps = steps.clamp(2, MAX_RAMP_STEPS as i64) as usize;
    let colors = palette::ramp_between(state.editor.primary, state.editor.secondary, steps);
    let with = palette::with_ramp(palette, &colors).ok_or_else(|| {
        CommandError::failed("The primary and the secondary colour are the same.")
    })?;
    set_palette(state, "Generate Ramp", with)
}

fn palette_replace(state: &mut AppState, params: &Params) -> CommandResult {
    let from = params.color("from").unwrap_or(state.editor.primary);
    let to = params.color("to").unwrap_or(state.editor.secondary);
    if from.normalized() == to.normalized() {
        return Err(CommandError::failed(
            "Both colours are the same: there is nothing to replace.",
        ));
    }
    let changed = transact(state, "Replace Colour", |tx| {
        edit::replace_color(tx, from, to)
    })?;
    state.log.push(format!(
        "Replaced {} with {} in {changed} pixels",
        from.to_hex(),
        to.to_hex()
    ));
    Ok(())
}

fn palette_reduce(state: &mut AppState, _: &Params) -> CommandResult {
    let changed = transact(state, "Reduce to Palette", edit::reduce_to_palette)?;
    state
        .log
        .push(format!("Changed {changed} pixels to palette colours"));
    Ok(())
}

fn add_reference(state: &mut AppState, params: &Params) -> CommandResult {
    let Some(path) = params.text("path").map(PathBuf::from) else {
        state.file_request = Some(FileRequest::ReferenceImage);
        return Ok(());
    };
    let could_not = |err: &dyn std::fmt::Display| {
        CommandError::failed(format!("Could not read {}: {err}", path.display()))
    };
    let bytes = std::fs::read(&path).map_err(|err| could_not(&err))?;
    let image = decode_png(&bytes).map_err(|err| could_not(&err))?;
    // It is shown as a single texture, like the canvas.
    if image.width() > MAX_SPRITE_SIZE || image.height() > MAX_SPRITE_SIZE {
        return Err(CommandError::failed(format!(
            "The picture is {} × {} pixels; a reference can be at most {MAX_SPRITE_SIZE} on a \
             side.",
            image.width(),
            image.height()
        )));
    }
    let name = path.file_stem().map_or_else(
        || "Reference".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );

    // On top of everything, so it is seen whatever the layers hold.
    let top = Slot::new(None, sprite(state)?.layers().len());
    let id = transact(state, "Add Reference Layer", |tx| {
        tx.add_reference_layer(top, &name, image)
    })?;
    if let Some(document) = state.editor.document_mut() {
        document.set_active_layer(id);
    }
    Ok(())
}

pub fn register(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Color, Int, Text};

    // Inks and the stroke pipeline.
    registry.register(Command::new("tool.set_ink", "Set Ink", set_ink).param("ink", Text))?;
    for ink in InkKind::ALL {
        registry.register(Command::new(
            format!("ink.{}", ink.id()),
            format!("Ink: {}", ink.name()),
            move |state: &mut AppState, _| {
                state.editor.options.ink = ink;
                Ok(())
            },
        ))?;
    }
    registry.register(
        Command::new("tool.set_pattern", "Set Dither Pattern", set_pattern).param("pattern", Text),
    )?;
    registry.register(
        Command::new(
            "tool.set_density",
            "Set Dither Density",
            |state: &mut AppState, params| {
                let density = params.int("density").unwrap_or(50);
                state.editor.options.dither_density =
                    density.clamp(0, i64::from(MAX_DENSITY)) as u8;
                Ok(())
            },
        )
        .param("density", Int),
    )?;
    registry.register(
        Command::new(
            "pattern.from_selection",
            "Dither Pattern from Selection",
            pattern_from_selection,
        )
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "brush.from_selection",
            "Brush from Selection",
            brush_from_selection,
        )
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "tool.set_pixel_perfect",
            "Toggle Pixel-Perfect Lines",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.pixel_perfect = params.bool("on").unwrap_or(!options.pixel_perfect);
                Ok(())
            },
        )
        .optional_param("on", Bool),
    )?;
    registry.register(
        Command::new(
            "view.toggle_tiled",
            "Toggle Tiled Mode",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.tiled = params.bool("on").unwrap_or(!options.tiled);
                Ok(())
            },
        )
        .optional_param("on", Bool),
    )?;

    // Symmetry.
    registry.register(
        Command::new("symmetry.set", "Set Symmetry", symmetry_set)
            .param("mode", Text)
            .optional_param("count", Int)
            .enabled_when(has_document),
    )?;
    let modes = [
        ("symmetry.off", "Symmetry: Off", Symmetry::Off),
        (
            "symmetry.horizontal",
            "Symmetry: Horizontal",
            Symmetry::Horizontal,
        ),
        (
            "symmetry.vertical",
            "Symmetry: Vertical",
            Symmetry::Vertical,
        ),
        ("symmetry.both", "Symmetry: Both", Symmetry::Both),
    ];
    for (id, name, symmetry) in modes {
        registry.register(
            Command::new(id, name, move |state: &mut AppState, _| {
                set_symmetry(state, symmetry)
            })
            .enabled_when(has_document),
        )?;
    }
    registry.register(
        Command::new(
            "symmetry.radial",
            "Symmetry: Radial",
            |state: &mut AppState, params| {
                let count = radial_count(state, params);
                set_symmetry(state, Symmetry::Radial(count))
            },
        )
        .optional_param("count", Int)
        .enabled_when(has_document),
    )?;

    // Palette.
    registry.register(
        Command::new("palette.import", "Import Palette…", palette_import)
            .optional_param("path", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("palette.export", "Export Palette…", palette_export)
            .optional_param("path", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.sort_hue",
            "Sort Palette by Hue",
            |state: &mut AppState, _| palette_sort(state, "Sort Palette by Hue", SortKey::Hue),
        )
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.sort_brightness",
            "Sort Palette by Brightness",
            |state: &mut AppState, _| {
                palette_sort(state, "Sort Palette by Brightness", SortKey::Brightness)
            },
        )
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.add_color",
            "Add Primary Colour to Palette",
            palette_add,
        )
        .optional_param("color", Color)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.remove_colors",
            "Remove Selected Colours from Palette",
            palette_remove,
        )
        .optional_param("indices", Text)
        .enabled_when(has_colors_selected),
    )?;
    registry.register(
        Command::new(
            "palette.new_ramp",
            "New Ramp from Selected Colours",
            palette_new_ramp,
        )
        .optional_param("indices", Text)
        .enabled_when(has_colors_selected),
    )?;
    registry.register(
        Command::new("palette.delete_ramp", "Delete Ramp", palette_delete_ramp)
            .param("ramp", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.generate_ramp",
            "Generate Ramp from Primary to Secondary…",
            palette_generate_ramp,
        )
        .optional_param("steps", Int)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "palette.replace_color",
            "Replace Primary with Secondary Colour in Sprite",
            palette_replace,
        )
        .optional_param("from", Color)
        .optional_param("to", Color)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("palette.reduce", "Reduce Sprite to Palette", palette_reduce)
            .enabled_when(has_document),
    )?;

    // Reference layers.
    registry.register(
        Command::new("layer.add_reference", "New Reference Layer…", add_reference)
            .optional_param("path", Text)
            .enabled_when(has_document),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{Fixture, RED};
    use crate::config::TempFile;
    use acms_core::geom::Rect;
    use acms_core::image::Image;
    use acms_core::selection::Mask;

    const GREEN: Rgba8 = Rgba8::rgb(0, 255, 0);

    fn text(file: &TempFile) -> String {
        file.path().to_string_lossy().into_owned()
    }

    fn with_path(file: &TempFile) -> Params {
        Params::new().with("path", text(file))
    }

    fn history(f: &Fixture) -> Vec<String> {
        f.document().history().map(str::to_owned).collect()
    }

    fn palette(f: &Fixture) -> &Palette {
        f.document().sprite().palette()
    }

    fn colors(f: &Fixture) -> Vec<Rgba8> {
        palette(f).colors().collect()
    }

    /// Takes the message a failed command left on screen.
    fn message(f: &mut Fixture) -> String {
        match f.state.dialog.take() {
            Some(Dialog::Message { text, .. }) => text,
            other => panic!("expected a message, found {other:?}"),
        }
    }

    fn select(f: &mut Fixture, rect: Rect) {
        let mask = Mask::from_rect(16, 16, rect);
        transact(&mut f.state, "Select", |tx| tx.set_selection(Some(mask))).unwrap();
    }

    #[test]
    fn ink_pattern_and_pipeline_settings() {
        let mut f = Fixture::new();
        f.run_with("tool.set_ink", Params::new().with("ink", "dither"));
        assert_eq!(f.state.editor.options.ink, InkKind::Dither);
        f.run("ink.shade");
        assert_eq!(f.state.editor.options.ink, InkKind::Shade);
        f.run_with("tool.set_ink", Params::new().with("ink", "glitter"));
        assert!(message(&mut f).contains("simple, dither, shade"));
        assert_eq!(f.state.editor.options.ink, InkKind::Shade);

        f.run_with("tool.set_pattern", Params::new().with("pattern", "bayer8"));
        assert_eq!(f.state.editor.options.pattern, Pattern::Bayer8);
        f.run_with("tool.set_pattern", Params::new().with("pattern", "custom"));
        assert!(message(&mut f).contains("no custom pattern yet"));
        f.run_with("tool.set_pattern", Params::new().with("pattern", "plaid"));
        assert!(message(&mut f).contains("checker"));
        assert_eq!(f.state.editor.options.pattern, Pattern::Bayer8);

        for (given, kept) in [(30_i64, 30_u8), (-5, 0), (900, 100)] {
            f.run_with("tool.set_density", Params::new().with("density", given));
            assert_eq!(f.state.editor.options.dither_density, kept);
        }

        f.run("tool.set_pixel_perfect");
        assert!(f.state.editor.options.pixel_perfect);
        f.run_with("tool.set_pixel_perfect", Params::new().with("on", true));
        assert!(f.state.editor.options.pixel_perfect);
        f.run("tool.set_pixel_perfect");
        assert!(!f.state.editor.options.pixel_perfect);

        f.run("view.toggle_tiled");
        assert!(f.state.editor.options.tiled);
        f.run_with("view.toggle_tiled", Params::new().with("on", false));
        assert!(!f.state.editor.options.tiled);
    }

    #[test]
    fn symmetry_is_set_per_sprite_and_mirrors_what_is_drawn() {
        let f = Fixture::new();
        assert!(!f.is_enabled("symmetry.horizontal"));

        let mut f = Fixture::with_sprite();
        let symmetry = |f: &Fixture| f.document().sprite().symmetry();
        f.run("symmetry.horizontal");
        assert_eq!(symmetry(&f), Symmetry::Horizontal);
        // A way of working, not a change to the picture.
        assert!(history(&f).is_empty());
        assert!(!f.state.has_unsaved_changes());

        f.draw_line(3);
        assert_eq!((f.pixel(2, 3), f.pixel(13, 3)), (RED, RED));
        assert_eq!(history(&f), ["Pencil"]);

        f.run("symmetry.radial");
        assert_eq!(symmetry(&f), Symmetry::Radial(6));
        let radial = |count: i64| Params::new().with("mode", "radial").with("count", count);
        f.run_with("symmetry.set", radial(5));
        assert_eq!(symmetry(&f), Symmetry::Radial(5));
        f.run_with("symmetry.set", radial(500));
        assert_eq!(symmetry(&f), Symmetry::Radial(16));
        // The count is remembered for the next time radial is switched on.
        f.run("symmetry.off");
        assert_eq!(symmetry(&f), Symmetry::Off);
        f.run("symmetry.radial");
        assert_eq!(symmetry(&f), Symmetry::Radial(16));
        f.run("symmetry.vertical");
        assert_eq!(symmetry(&f), Symmetry::Vertical);
        f.run("symmetry.both");
        assert_eq!(symmetry(&f), Symmetry::Both);

        f.run_with("symmetry.set", Params::new().with("mode", "spiral"));
        assert!(message(&mut f).contains("not a symmetry"));
        assert_eq!(symmetry(&f), Symmetry::Both);

        // Another sprite has its own setting.
        f.run_with(
            "file.new",
            Params::new().with("width", 8_u32).with("height", 8_u32),
        );
        assert_eq!(symmetry(&f), Symmetry::Off);
    }

    #[test]
    fn symmetry_pixel_perfect_and_dither_are_one_undo_step_through_commands() {
        use acms_core::geom::Point;
        use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

        let mut f = Fixture::with_sprite();
        f.run("symmetry.both");
        f.run_with("tool.set_pixel_perfect", Params::new().with("on", true));
        f.run("ink.dither");
        f.run_with("tool.set_pattern", Params::new().with("pattern", "checker"));
        f.state.editor.primary = RED;
        f.state.editor.secondary = GREEN;

        let event = |(x, y): (i32, i32)| PointerEvent {
            position: Point::new(x, y),
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        };
        let path = [(1, 1), (2, 1), (2, 2), (3, 2), (3, 3), (6, 3)];
        f.state.editor.pointer_down(event(path[0]));
        for point in &path[1..] {
            f.state.editor.pointer_move(event(*point));
        }
        f.state.editor.pointer_up(event(path[5]));

        assert_eq!(history(&f), ["Pencil"]);
        // The stairs became a diagonal, in two colours, in all four corners.
        assert_eq!(f.pixel(2, 1), Rgba8::TRANSPARENT);
        assert_eq!(f.pixel(3, 2), Rgba8::TRANSPARENT);
        let drawn = [
            (1, 1),
            (2, 2),
            (4, 3),
            (14, 1),
            (11, 3),
            (1, 14),
            (4, 12),
            (13, 13),
        ];
        for (x, y) in drawn {
            let expected = if (x + y) % 2 == 0 { RED } else { GREEN };
            assert_eq!(f.pixel(x, y), expected, "({x}, {y})");
        }
        f.run("edit.undo");
        assert_eq!(f.document().sprite().cels().count(), 0);
    }

    #[test]
    fn a_brush_and_a_pattern_are_made_from_the_selection() {
        let mut f = Fixture::with_sprite();
        f.run("brush.from_selection");
        assert!(message(&mut f).contains("nothing in it"));
        assert!(!f.state.editor.brush.is_custom());

        f.draw_line(3);
        select(&mut f, Rect::new(2, 2, 3, 3));
        f.run("brush.from_selection");
        let image = f.state.editor.brush.image().unwrap().clone();
        assert_eq!((image.width(), image.height()), (3, 1));

        // Picking a plain shape again drops the custom brush.
        f.run_with("brush.set_shape", Params::new().with("shape", "square"));
        assert!(!f.state.editor.brush.is_custom());

        f.run("pattern.from_selection");
        assert_eq!(f.state.editor.options.ink, InkKind::Dither);
        let custom = f.state.editor.options.pattern.clone();
        assert_eq!(custom.id(), "custom");
        // Row 1 of the 3×3 tile is the line.
        assert!(custom.picks_first(0, 1, 0) && !custom.picks_first(0, 0, 0));

        // It can be put away and taken out again.
        f.run_with("tool.set_pattern", Params::new().with("pattern", "bayer2"));
        f.run_with("tool.set_pattern", Params::new().with("pattern", "custom"));
        assert_eq!(f.state.editor.options.pattern, custom);

        select(&mut f, Rect::new(0, 0, 16, 16));
        f.run("brush.from_selection");
        assert!(f.state.editor.brush.is_custom());
        let mut big = Fixture::new();
        big.run_with(
            "file.new",
            Params::new().with("width", 100_u32).with("height", 8_u32),
        );
        big.run("pattern.from_selection");
        assert!(message(&mut big).contains("nothing in it"));
    }

    #[test]
    fn colours_and_ramps_are_added_and_removed_as_undo_steps() {
        let mut f = Fixture::with_sprite();
        let start = colors(&f);
        assert!(!f.is_enabled("palette.new_ramp"));
        assert!(!f.is_enabled("palette.remove_colors"));

        let orange = Rgba8::rgb(255, 128, 1);
        f.state.editor.primary = orange;
        f.run("palette.add_color");
        assert_eq!(colors(&f).last(), Some(&orange));
        // Already there: nothing happens.
        f.run("palette.add_color");
        f.run_with(
            "palette.add_color",
            Params::new().with("color", "#00000000"),
        );
        assert!(message(&mut f).contains("transparent"));
        assert_eq!(colors(&f).len(), start.len() + 1);
        assert_eq!(history(&f), ["Add Colour"]);

        // A ramp from the swatches picked in the panel, in the order they were picked.
        f.state.swatch_selection = vec![6, 3, 0];
        f.run("palette.new_ramp");
        assert_eq!(palette(&f).ramps(), [vec![start[6], start[3], start[0]]]);
        f.run_with("palette.new_ramp", Params::new().with("indices", "1, 2"));
        assert_eq!(palette(&f).ramps()[1], [start[1], start[2]]);
        f.run_with("palette.new_ramp", Params::new().with("indices", "4"));
        assert!(message(&mut f).contains("at least two"));
        f.run_with("palette.new_ramp", Params::new().with("indices", "one"));
        assert!(message(&mut f).contains("not a position"));

        f.run_with("palette.delete_ramp", Params::new().with("ramp", 0_i64));
        assert_eq!(palette(&f).ramps(), [vec![start[1], start[2]]]);
        f.run_with("palette.delete_ramp", Params::new().with("ramp", 7_i64));
        assert!(message(&mut f).contains("no such ramp"));

        // Removing a colour takes it out of the ramps as well.
        f.state.swatch_selection = vec![1];
        f.run("palette.remove_colors");
        assert_eq!(colors(&f).len(), start.len());
        assert!(palette(&f).ramps().is_empty());
        assert!(f.state.swatch_selection.is_empty());

        assert_eq!(
            history(&f),
            [
                "Add Colour",
                "New Ramp",
                "New Ramp",
                "Delete Ramp",
                "Remove Colours"
            ]
        );
        for _ in 0..5 {
            f.run("edit.undo");
        }
        assert_eq!(colors(&f), start);
    }

    #[test]
    fn a_ramp_is_generated_between_the_two_colours() {
        let mut f = Fixture::with_sprite();
        let before = colors(&f).len();
        f.run("palette.generate_ramp");
        assert_eq!(f.state.dialog.take(), Some(Dialog::Ramp { steps: 5 }));
        assert!(palette(&f).ramps().is_empty());

        // Black to white: both ends are in the default palette, the two between are new.
        f.run_with("palette.generate_ramp", Params::new().with("steps", 4_i64));
        let ramp = palette(&f).ramps()[0].clone();
        assert_eq!(ramp.len(), 4);
        assert_eq!((ramp[0], ramp[3]), (Rgba8::BLACK, Rgba8::WHITE));
        assert_eq!(colors(&f).len(), before + 2);
        assert_eq!(history(&f), ["Generate Ramp"]);

        f.state.editor.secondary = f.state.editor.primary;
        f.run_with("palette.generate_ramp", Params::new().with("steps", 4_i64));
        assert!(message(&mut f).contains("the same"));
    }

    #[test]
    fn sorting_the_palette() {
        let mut f = Fixture::with_sprite();
        let start = colors(&f);
        f.state.swatch_selection = vec![2, 3];
        f.run("palette.sort_brightness");
        let sorted = colors(&f);
        assert_eq!(sorted.len(), start.len());
        assert_eq!(sorted.first(), Some(&Rgba8::BLACK));
        assert_eq!(sorted.last(), Some(&Rgba8::WHITE));
        assert!(f.state.swatch_selection.is_empty());

        f.run("palette.sort_hue");
        // Black and white have no hue and come first; the rest go round the colour wheel.
        assert_eq!(colors(&f)[..2], [Rgba8::BLACK, Rgba8::WHITE]);
        let hues: Vec<u32> = colors(&f)[2..]
            .iter()
            .filter_map(|c| palette::hue(*c))
            .collect();
        assert_eq!(hues.len(), start.len() - 2);
        assert!(hues.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(
            history(&f),
            ["Sort Palette by Brightness", "Sort Palette by Hue"]
        );
        f.run("edit.undo");
        f.run("edit.undo");
        assert_eq!(colors(&f), start);
    }

    #[test]
    fn replacing_a_colour_and_reducing_to_the_palette() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        f.state.editor.secondary = GREEN;
        f.run("palette.replace_color");
        assert_eq!(f.pixel(5, 3), GREEN);
        assert_eq!(
            f.state.log.last().map(String::as_str),
            Some("Replaced #FF0000 with #00FF00 in 9 pixels")
        );
        f.run("edit.undo");
        assert_eq!(f.pixel(5, 3), RED);

        f.run_with(
            "palette.replace_color",
            Params::new().with("from", RED).with("to", RED),
        );
        assert!(message(&mut f).contains("nothing to replace"));

        // Pure red is not in the default palette; its red is the nearest.
        f.run("palette.reduce");
        let nearest = palette::nearest(palette(&f), RED).unwrap();
        assert_ne!(nearest, RED);
        assert_eq!(f.pixel(5, 3), nearest);
        assert_eq!(
            history(&f).last().map(String::as_str),
            Some("Reduce to Palette")
        );
        f.run("edit.undo");
        assert_eq!(f.pixel(5, 3), RED);
    }

    #[test]
    fn palettes_are_exported_and_imported() {
        let mut f = Fixture::with_sprite();
        let start = colors(&f);
        f.run("palette.export");
        assert_eq!(
            f.state.file_request.take(),
            Some(FileRequest::ExportPalette)
        );
        f.run("palette.import");
        assert_eq!(
            f.state.file_request.take(),
            Some(FileRequest::ImportPalette)
        );

        let gpl = TempFile::path_only("export.gpl");
        f.run_with("palette.export", with_path(&gpl));
        let written = std::fs::read_to_string(gpl.path()).unwrap();
        assert!(written.starts_with("GIMP Palette"));
        assert!(history(&f).is_empty());

        let hex = TempFile::new("import.hex", "ff0000\n00ff00\n");
        f.run_with("palette.import", with_path(&hex));
        assert_eq!(colors(&f), [RED, GREEN]);
        assert_eq!(history(&f), ["Import Palette"]);
        assert!(
            f.state
                .log
                .last()
                .unwrap()
                .starts_with("Imported 2 colours")
        );

        // What was exported comes back as it was.
        f.run_with("palette.import", with_path(&gpl));
        assert_eq!(colors(&f), start);

        let odd = TempFile::new("import.txt", "ff0000");
        f.run_with("palette.import", with_path(&odd));
        assert!(message(&mut f).contains(".gpl, .hex, .pal and .png"));
        f.run_with("palette.export", with_path(&odd));
        assert!(message(&mut f).contains("Could not write"));
        let broken = TempFile::new("broken.hex", "ff0000\nwhat\n");
        f.run_with("palette.import", with_path(&broken));
        assert!(message(&mut f).contains("line 2"));
        assert_eq!(colors(&f), start);
    }

    fn picture(name: &str, width: u32, height: u32) -> TempFile {
        let pixels = vec![Rgba8::rgb(10, 200, 90); (width * height) as usize];
        let image = Image::from_pixels(width, height, pixels).unwrap();
        let mut bytes = Vec::new();
        acms_io::png::encode_png(&image, &mut bytes).unwrap();
        let file = TempFile::path_only(name);
        std::fs::write(file.path(), bytes).unwrap();
        file
    }

    #[test]
    fn a_reference_layer_is_added_shown_and_saved_but_never_drawn_on() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        f.run("layer.add_reference");
        assert_eq!(
            f.state.file_request.take(),
            Some(FileRequest::ReferenceImage)
        );

        let file = picture("reference.png", 8, 4);
        f.run_with("layer.add_reference", with_path(&file));
        let document = f.document();
        let top = document.sprite().layers().last().unwrap().clone();
        assert_eq!(document.active_layer(), top.id);
        assert!(top.name.ends_with("reference"));
        let reference = top.as_reference().unwrap();
        assert_eq!(reference.rect, Rect::new(0, 4, 16, 8));
        assert_eq!(
            history(&f).last().map(String::as_str),
            Some("Add Reference Layer")
        );

        // Its opacity changes like a layer's; it has no blend mode and cannot be drawn on.
        f.run_with("layer.set_opacity", Params::new().with("opacity", 60_u32));
        let opacity = |f: &Fixture| {
            let layer = f.document().sprite().layer(top.id).unwrap();
            layer.as_reference().unwrap().opacity
        };
        assert_eq!(opacity(&f), 60);
        f.run_with("layer.set_blend", Params::new().with("blend", "multiply"));
        assert!(message(&mut f).contains("not an image layer"));
        assert!(f.state.editor.draw_block().is_some());
        let steps = history(&f).len();
        f.draw_line(8);
        assert_eq!(history(&f).len(), steps);
        assert!(!f.is_enabled("layer.merge_down"));

        // It is in the saved file, and not in an exported picture.
        let saved = TempFile::path_only("reference.acms");
        f.run_with("file.save_as", with_path(&saved));
        let exported = TempFile::path_only("reference-export.png");
        f.run_with("file.export_png", with_path(&exported));
        let flat = decode_png(&std::fs::read(exported.path()).unwrap()).unwrap();
        assert_eq!(flat.get(5, 3), RED);
        assert_eq!(flat.get(5, 8), Rgba8::TRANSPARENT);

        let sprite = f.document().sprite().clone();
        f.run("file.close");
        f.run_with("file.open", with_path(&saved));
        assert_eq!(*f.document().sprite(), sprite);

        f.run("edit.undo");
        let missing = TempFile::path_only("no-such.png");
        f.run_with("layer.add_reference", with_path(&missing));
        assert!(message(&mut f).contains("Could not read"));
        let not_a_picture = TempFile::new("not-a-picture.png", "hello");
        f.run_with("layer.add_reference", with_path(&not_a_picture));
        assert!(message(&mut f).contains("Could not read"));
    }
}
