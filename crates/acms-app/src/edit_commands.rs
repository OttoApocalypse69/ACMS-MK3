//! Commands for the clipboard, the selection, transforms, the canvas size, tool settings and
//! switching between open sprites.

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::edit::{self, Clip, Transform};
use acms_core::editor::DocId;
use acms_core::geom::{Point, Rect};
use acms_core::selection::Mask;
use acms_core::sprite::{LayerId, Sprite};

use crate::clipboard;
use crate::layer_commands::transact;
use crate::state::{Anchor, AppState, Dialog, PendingCommand, SizePurpose};

type CommandResult = Result<(), CommandError>;

fn has_document(state: &AppState) -> bool {
    state.editor.document().is_some()
}

fn has_selection(state: &AppState) -> bool {
    state
        .editor
        .document()
        .is_some_and(|document| document.sprite().selection().is_some())
}

/// The active sprite with its selected layer and frame.
fn target(state: &AppState) -> Result<(&Sprite, LayerId, usize), CommandError> {
    let document = state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))?;
    Ok((
        document.sprite(),
        document.active_layer(),
        document.active_frame(),
    ))
}

fn copy(state: &mut AppState, _: &Params) -> CommandResult {
    let (sprite, layer, frame) = target(state)?;
    let clip = edit::copy(sprite, layer, frame)
        .ok_or_else(|| CommandError::failed("There is nothing here to copy."))?;
    if state.system_clipboard {
        clipboard::write_image(&clip.image, &mut state.log);
    }
    state.editor.clipboard = Some(clip);
    Ok(())
}

fn cut(state: &mut AppState, params: &Params) -> CommandResult {
    copy(state, params)?;
    let (_, layer, frame) = target(state)?;
    transact(state, "Cut", |tx| edit::clear(tx, layer, frame))
}

fn clear(state: &mut AppState, _: &Params) -> CommandResult {
    let (_, layer, frame) = target(state)?;
    transact(state, "Clear", |tx| edit::clear(tx, layer, frame))
}

fn paste(state: &mut AppState, _: &Params) -> CommandResult {
    let (_, layer, frame) = target(state)?;
    let own = state.editor.clipboard.clone();
    let from_system = state.system_clipboard.then(clipboard::read_image).flatten();
    // What this app copied itself still knows where it came from; anything else on the
    // system clipboard is newer and goes to the top-left corner.
    let clip = match (from_system, own) {
        (Some(image), Some(own)) if own.image == image => own,
        (Some(image), _) => Clip {
            image,
            origin: Point::new(0, 0),
        },
        (None, Some(own)) => own,
        (None, None) => return Err(CommandError::failed("The clipboard holds no image.")),
    };
    transact(state, "Paste", |tx| edit::paste(tx, layer, frame, &clip))
}

fn select_all(state: &mut AppState, _: &Params) -> CommandResult {
    let (sprite, ..) = target(state)?;
    let all = Mask::full(sprite.width(), sprite.height());
    transact(state, "Select All", |tx| tx.set_selection(Some(all)))
}

fn deselect(state: &mut AppState, _: &Params) -> CommandResult {
    transact(state, "Deselect", |tx| tx.set_selection(None))
}

fn invert_selection(state: &mut AppState, _: &Params) -> CommandResult {
    let (sprite, ..) = target(state)?;
    let inverted = match sprite.selection() {
        Some(mask) => mask.inverted(),
        None => Mask::full(sprite.width(), sprite.height()),
    };
    transact(state, "Invert Selection", |tx| {
        tx.set_selection(Some(inverted))
    })
}

fn transform(state: &mut AppState, name: &str, transform: Transform) -> CommandResult {
    let (_, layer, frame) = target(state)?;
    transact(state, name, |tx| {
        edit::transform(tx, layer, frame, transform)
    })
}

/// The `width` and `height` arguments, if both were given.
fn size_args(params: &Params) -> Option<(u32, u32)> {
    let side = |name| params.int(name).map(|n| u32::try_from(n).unwrap_or(0));
    Some((side("width")?, side("height")?))
}

fn ask_size(state: &mut AppState, purpose: SizePurpose, size: Rect) {
    state.dialog = Some(Dialog::Size {
        purpose,
        width: size.w.max(1),
        height: size.h.max(1),
        anchor: Anchor::default(),
    });
}

fn scale_selection(state: &mut AppState, params: &Params) -> CommandResult {
    let (sprite, layer, frame) = target(state)?;
    let Some((width, height)) = size_args(params) else {
        let region = edit::region(sprite, layer, frame).map_or(Rect::EMPTY, |mask| mask.bounds());
        ask_size(state, SizePurpose::ScaleSelection, region);
        return Ok(());
    };
    if width == 0 || height == 0 {
        return Err(CommandError::failed("The size must be at least 1 × 1."));
    }
    transform(state, "Scale Selection", Transform::Scale { width, height })
}

fn canvas_size(state: &mut AppState, params: &Params) -> CommandResult {
    let (sprite, ..) = target(state)?;
    let old = sprite.bounds();
    let Some((width, height)) = size_args(params) else {
        ask_size(state, SizePurpose::CanvasSize, old);
        return Ok(());
    };
    let anchor = match params.text("anchor") {
        None => Anchor::default(),
        Some(id) => Anchor::from_id(id)
            .ok_or_else(|| CommandError::failed(format!("\"{id}\" is not an anchor.")))?,
    };
    let (dx, dy) = anchor.offset(width as i32 - old.w as i32, height as i32 - old.h as i32);
    transact(state, "Canvas Size", |tx| {
        tx.resize_canvas(width, height, dx, dy)
    })?;
    state.view.fit_pending = true;
    Ok(())
}

fn crop(state: &mut AppState, _: &Params) -> CommandResult {
    let (sprite, ..) = target(state)?;
    let bounds = sprite
        .selection()
        .map(|mask| mask.bounds())
        .ok_or_else(|| CommandError::failed("Select the area to keep first."))?;
    transact(state, "Crop to Selection", |tx| {
        tx.resize_canvas(bounds.w, bounds.h, -bounds.x, -bounds.y)
    })?;
    state.view.fit_pending = true;
    Ok(())
}

fn sprite_size(state: &mut AppState, params: &Params) -> CommandResult {
    let (sprite, ..) = target(state)?;
    let Some((width, height)) = size_args(params) else {
        let old = sprite.bounds();
        ask_size(state, SizePurpose::SpriteSize, old);
        return Ok(());
    };
    transact(state, "Sprite Size", |tx| tx.scale_sprite(width, height))?;
    state.view.fit_pending = true;
    Ok(())
}

fn close(state: &mut AppState, params: &Params) -> CommandResult {
    if state.has_unsaved_changes() && params.bool("discard") != Some(true) {
        state.dialog = Some(Dialog::ConfirmDiscard {
            then: PendingCommand {
                id: "file.close".to_owned(),
                params: params.clone().with("discard", true),
            },
        });
        return Ok(());
    }
    state.close_active_document();
    Ok(())
}

fn activate(state: &mut AppState, params: &Params) -> CommandResult {
    let id = DocId(u64::try_from(params.int("doc").unwrap_or(0)).unwrap_or(0));
    if state.activate_document(id) {
        Ok(())
    } else {
        Err(CommandError::failed("That sprite is not open."))
    }
}

/// Switches to the sprite `step` tabs to the right (or left, if negative), wrapping around.
fn cycle(state: &mut AppState, step: isize) -> CommandResult {
    let ids: Vec<DocId> = state.editor.documents().map(|(id, _)| id).collect();
    let current = state.editor.active_document();
    if let Some(index) = ids.iter().position(|id| Some(*id) == current) {
        let next = (index as isize + step).rem_euclid(ids.len() as isize) as usize;
        state.activate_document(ids[next]);
    }
    Ok(())
}

pub fn register(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Int, Text};

    registry.register(
        Command::new("file.close", "Close", close)
            .optional_param("discard", Bool)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("doc.activate", "Switch to Sprite", activate)
            .param("doc", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("doc.next", "Next Sprite", |state: &mut AppState, _| {
            cycle(state, 1)
        })
        .enabled_when(|state| state.editor.documents().len() > 1),
    )?;
    registry.register(
        Command::new(
            "doc.previous",
            "Previous Sprite",
            |state: &mut AppState, _| cycle(state, -1),
        )
        .enabled_when(|state| state.editor.documents().len() > 1),
    )?;

    registry.register(Command::new("edit.cut", "Cut", cut).enabled_when(has_document))?;
    registry.register(Command::new("edit.copy", "Copy", copy).enabled_when(has_document))?;
    registry.register(Command::new("edit.paste", "Paste", paste).enabled_when(has_document))?;
    registry.register(Command::new("edit.clear", "Delete", clear).enabled_when(has_document))?;

    registry.register(
        Command::new("select.all", "Select All", select_all).enabled_when(has_document),
    )?;
    registry
        .register(Command::new("select.none", "Deselect", deselect).enabled_when(has_selection))?;
    registry.register(
        Command::new("select.invert", "Invert Selection", invert_selection)
            .enabled_when(has_document),
    )?;

    let transforms = [
        (
            "selection.flip_horizontal",
            "Flip Horizontal",
            Transform::FlipHorizontal,
        ),
        (
            "selection.flip_vertical",
            "Flip Vertical",
            Transform::FlipVertical,
        ),
        (
            "selection.rotate_cw",
            "Rotate 90° Clockwise",
            Transform::RotateClockwise,
        ),
        (
            "selection.rotate_ccw",
            "Rotate 90° Anticlockwise",
            Transform::RotateAnticlockwise,
        ),
    ];
    for (id, name, kind) in transforms {
        registry.register(
            Command::new(id, name, move |state: &mut AppState, _| {
                transform(state, name, kind)
            })
            .enabled_when(has_document),
        )?;
    }
    registry.register(
        Command::new("selection.scale", "Scale Selection…", scale_selection)
            .optional_param("width", Int)
            .optional_param("height", Int)
            .enabled_when(has_document),
    )?;

    registry.register(
        Command::new("sprite.canvas_size", "Canvas Size…", canvas_size)
            .optional_param("width", Int)
            .optional_param("height", Int)
            .optional_param("anchor", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("sprite.crop", "Crop to Selection", crop).enabled_when(has_selection),
    )?;
    registry.register(
        Command::new("sprite.size", "Sprite Size…", sprite_size)
            .optional_param("width", Int)
            .optional_param("height", Int)
            .enabled_when(has_document),
    )?;

    // Tool settings. Without an argument the on / off ones toggle.
    registry.register(
        Command::new(
            "tool.set_filled",
            "Toggle Filled Shapes",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.shape_filled = params.bool("filled").unwrap_or(!options.shape_filled);
                Ok(())
            },
        )
        .optional_param("filled", Bool),
    )?;
    registry.register(
        Command::new(
            "tool.set_contiguous",
            "Toggle Contiguous Fill",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.fill_contiguous = params
                    .bool("contiguous")
                    .unwrap_or(!options.fill_contiguous);
                Ok(())
            },
        )
        .optional_param("contiguous", Bool),
    )?;
    registry.register(
        Command::new(
            "tool.set_tolerance",
            "Set Colour Tolerance",
            |state: &mut AppState, params| {
                let tolerance = params.int("tolerance").unwrap_or(0).clamp(0, 255);
                state.editor.options.tolerance = tolerance as u8;
                Ok(())
            },
        )
        .param("tolerance", Int),
    )?;
    registry.register(
        Command::new(
            "tool.set_dither",
            "Toggle Dithered Gradient",
            |state: &mut AppState, params| {
                let options = &mut state.editor.options;
                options.gradient_dither = params.bool("dither").unwrap_or(!options.gradient_dither);
                Ok(())
            },
        )
        .optional_param("dither", Bool),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{Fixture, RED};
    use acms_core::color::Rgba8;

    fn select(f: &mut Fixture, rect: Rect) {
        let mask = Mask::from_rect(16, 16, rect);
        transact(&mut f.state, "Select", |tx| tx.set_selection(Some(mask))).unwrap();
    }

    fn selection(f: &Fixture) -> Option<Rect> {
        f.document().sprite().selection().map(|mask| mask.bounds())
    }

    fn size(f: &Fixture) -> (u32, u32) {
        let sprite = f.document().sprite();
        (sprite.width(), sprite.height())
    }

    fn history(f: &Fixture) -> Vec<String> {
        f.document().history().map(str::to_owned).collect()
    }

    fn wh(width: u32, height: u32) -> Params {
        Params::new().with("width", width).with("height", height)
    }

    #[test]
    fn cut_copy_paste_and_delete() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        select(&mut f, Rect::new(2, 3, 4, 1));

        f.run("edit.copy");
        assert_eq!(
            f.state.editor.clipboard.as_ref().unwrap().origin,
            Point::new(2, 3)
        );
        f.run("edit.cut");
        assert_eq!((f.pixel(3, 3), f.pixel(7, 3)), (Rgba8::TRANSPARENT, RED));
        f.run("edit.paste");
        assert_eq!(f.pixel(3, 3), RED);
        assert_eq!(selection(&f), Some(Rect::new(2, 3, 4, 1)));

        f.run("select.none");
        f.run("edit.clear");
        assert_eq!(f.document().sprite().cels().count(), 0);
        assert_eq!(
            history(&f)[2..],
            ["Cut", "Paste", "Deselect", "Clear"].map(str::to_owned)
        );

        // Nothing visible to copy, and an empty clipboard, are said out loud.
        f.run("edit.copy");
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
        f.state.editor.clipboard = None;
        f.run("edit.paste");
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
    }

    #[test]
    fn the_clipboard_is_shared_between_open_sprites() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        f.run("edit.copy");
        f.run_with("file.new", wh(16, 16));
        f.run("edit.paste");
        assert_eq!(f.pixel(5, 3), RED);
    }

    #[test]
    fn select_all_none_and_invert() {
        let mut f = Fixture::with_sprite();
        assert!(!f.is_enabled("select.none"));
        f.run("select.all");
        assert_eq!(selection(&f), Some(Rect::from_size(16, 16)));
        f.run("select.none");
        assert_eq!(selection(&f), None);

        select(&mut f, Rect::new(0, 0, 16, 4));
        f.run("select.invert");
        assert_eq!(selection(&f), Some(Rect::new(0, 4, 16, 12)));
        // Inverting "nothing selected" selects everything.
        f.run("select.none");
        f.run("select.invert");
        assert_eq!(selection(&f), Some(Rect::from_size(16, 16)));
        f.run("edit.undo");
        assert_eq!(selection(&f), None);
    }

    #[test]
    fn flip_rotate_and_scale() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        let rect = |f: &Fixture| f.document().sprite().cel(LayerId(1), 0).unwrap().rect();
        assert_eq!(rect(&f), Rect::new(2, 3, 9, 1));

        f.run("selection.rotate_cw");
        assert_eq!(rect(&f), Rect::new(6, 0, 1, 8));
        f.run("edit.undo");
        f.run("selection.flip_horizontal");
        f.run("selection.flip_vertical");
        assert_eq!(rect(&f), Rect::new(2, 3, 9, 1));

        f.run("selection.scale");
        assert_eq!(
            f.state.dialog.take(),
            Some(Dialog::Size {
                purpose: SizePurpose::ScaleSelection,
                width: 9,
                height: 1,
                anchor: Anchor::TopLeft
            })
        );
        f.run_with("selection.scale", wh(3, 2));
        assert_eq!(rect(&f), Rect::new(2, 3, 3, 2));
        assert_eq!(
            history(&f).last().map(String::as_str),
            Some("Scale Selection")
        );
        f.run_with("selection.scale", wh(0, 2));
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
    }

    #[test]
    fn canvas_size_crop_and_sprite_size() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        f.run("sprite.canvas_size");
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Size {
                purpose: SizePurpose::CanvasSize,
                width: 16,
                height: 16,
                ..
            })
        ));

        // Growing around the centre moves the picture by half the growth.
        f.state.view.fit_pending = false;
        f.run_with("sprite.canvas_size", wh(20, 24).with("anchor", "center"));
        assert_eq!(size(&f), (20, 24));
        assert_eq!(f.pixel(4, 7), RED);
        assert!(f.state.view.fit_pending);
        f.run_with("sprite.canvas_size", wh(20, 24).with("anchor", "middle"));
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));

        assert!(!f.is_enabled("sprite.crop"));
        transact(&mut f.state, "Select", |tx| {
            tx.set_selection(Some(Mask::from_rect(20, 24, Rect::new(4, 6, 8, 4))))
        })
        .unwrap();
        f.run("sprite.crop");
        assert_eq!(size(&f), (8, 4));
        assert_eq!(f.pixel(0, 1), RED);

        f.run_with("sprite.size", wh(16, 8));
        assert_eq!(size(&f), (16, 8));
        assert_eq!((f.pixel(0, 2), f.pixel(1, 3)), (RED, RED));
        f.run_with("sprite.size", wh(0, 8));
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));

        f.run("edit.undo");
        f.run("edit.undo");
        f.run("edit.undo");
        f.run("edit.undo");
        assert_eq!(size(&f), (16, 16));
    }

    #[test]
    fn tool_settings() {
        let mut f = Fixture::new();
        f.run("tool.set_filled");
        f.run("tool.set_contiguous");
        f.run("tool.set_dither");
        f.run_with(
            "tool.set_tolerance",
            Params::new().with("tolerance", 300_i64),
        );
        let options = f.state.editor.options;
        assert!(options.shape_filled && !options.fill_contiguous && !options.gradient_dither);
        assert_eq!(options.tolerance, 255);
        f.run_with("tool.set_filled", Params::new().with("filled", false));
        assert!(!f.state.editor.options.shape_filled);
    }

    #[test]
    fn several_sprites_each_keep_their_file_view_and_unsaved_state() {
        let mut f = Fixture::with_sprite();
        let first = f.state.editor.active_document().unwrap();
        f.draw_line(3);
        f.state.view.zoom_by(2, egui::Vec2::ZERO);
        let zoom = f.state.view.zoom();
        assert!(f.state.has_unsaved_changes());

        f.run_with("file.new", wh(8, 8));
        let second = f.state.editor.active_document().unwrap();
        assert_eq!(f.state.editor.documents().len(), 2);
        assert!(!f.state.has_unsaved_changes());
        assert!(f.state.any_unsaved_changes());
        assert_ne!(f.state.view.zoom(), zoom);
        assert_eq!(f.state.title_of(first), "Untitled •");
        assert_eq!(f.state.title_of(second), "Untitled");

        f.run("doc.next");
        assert_eq!(f.state.editor.active_document(), Some(first));
        assert_eq!(f.state.view.zoom(), zoom);
        assert!(f.state.has_unsaved_changes());
        f.run("doc.previous");
        assert_eq!(f.state.editor.active_document(), Some(second));
        f.run_with("doc.activate", Params::new().with("doc", first.0 as i64));
        assert_eq!(f.state.editor.active_document(), Some(first));

        // Closing a changed sprite asks; an unchanged one just closes.
        f.run("file.close");
        assert!(matches!(
            f.state.dialog,
            Some(Dialog::ConfirmDiscard { .. })
        ));
        assert_eq!(f.state.editor.documents().len(), 2);
        f.confirm_pending();
        assert_eq!(f.state.editor.active_document(), Some(second));
        assert!(!f.state.any_unsaved_changes());
        f.run("file.close");
        assert!(f.state.editor.document().is_none());
        assert_eq!(f.state.document_title(), "Canvas");
    }
}
