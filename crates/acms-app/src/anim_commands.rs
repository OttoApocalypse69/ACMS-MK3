//! Commands for frames, cels, tags, playback, onion skin and the animation exports.

use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::compositor::MAX_ONION_FRAMES;
use acms_core::document::Document;
use acms_core::playback;
use acms_core::sprite::{Direction, LayerId, Sprite, SpriteError, Tag};
use acms_io::export::{
    SheetLayout, SheetOptions, export_gif_to_path, export_png_sequence, export_sheet,
};

use crate::commands::sentence;
use crate::layer_commands::transact;
use crate::state::{AppState, CelRange, Dialog, ExportKind, FileRequest, MAX_PREVIEW_SCALE};

type CommandResult = Result<(), CommandError>;

/// Colours new tags get, in turn.
const TAG_COLORS: [u32; 6] = [0x3B6FE0, 0xE8852B, 0x3FA34D, 0xD9529E, 0x8A4FD0, 0x2A9D9C];

fn has_document(state: &AppState) -> bool {
    state.editor.document().is_some()
}

fn has_frames(state: &AppState) -> bool {
    state
        .editor
        .document()
        .is_some_and(|document| document.sprite().frames().len() > 1)
}

fn failed(err: SpriteError) -> CommandError {
    CommandError::failed(sentence(err))
}

fn document(state: &AppState) -> Result<&Document, CommandError> {
    state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))
}

fn sprite(state: &AppState) -> Result<&Sprite, CommandError> {
    Ok(document(state)?.sprite())
}

/// A whole number argument as a position; one that cannot be a position becomes one that
/// does not exist.
fn index(params: &Params, name: &str) -> Option<usize> {
    params
        .int(name)
        .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
}

/// Shows a frame. Picking a frame by hand ends playback.
fn show_frame(state: &mut AppState, frame: usize) {
    state.stop_playback();
    if let Some(document) = state.editor.document_mut() {
        document.set_active_frame(frame);
    }
}

/// The frames a frame command is about: those selected in the timeline if the frame being
/// shown is among them, otherwise just that frame. First and last, both included.
fn target_frames(state: &AppState) -> Result<(usize, usize), CommandError> {
    let active = document(state)?.active_frame();
    Ok(match state.cel_range {
        Some(range) if range.contains(active) => (range.first(), range.last()),
        _ => (active, active),
    })
}

fn frame_new(state: &mut AppState, duplicate: bool) -> CommandResult {
    let active = document(state)?.active_frame();
    let name = if duplicate {
        "Duplicate Frame"
    } else {
        "New Frame"
    };
    transact(state, name, |tx| {
        tx.add_frame(active + 1, duplicate.then_some(active))
    })?;
    state.cel_range = None;
    show_frame(state, active + 1);
    Ok(())
}

fn frame_delete(state: &mut AppState, _: &Params) -> CommandResult {
    let (first, last) = target_frames(state)?;
    if last - first + 1 >= sprite(state)?.frames().len() {
        return Err(CommandError::failed(
            "A sprite needs at least one frame: not all of them can be deleted.",
        ));
    }
    transact(state, "Delete Frame", |tx| {
        (first..=last)
            .rev()
            .try_for_each(|frame| tx.remove_frame(frame))
    })?;
    state.cel_range = None;
    let count = sprite(state)?.frames().len();
    show_frame(state, first.min(count - 1));
    Ok(())
}

/// Shows the frame `step` places on, wrapping around at the ends.
fn frame_step(state: &mut AppState, step: isize) -> CommandResult {
    let document = document(state)?;
    let count = document.sprite().frames().len() as isize;
    let next = (document.active_frame() as isize + step).rem_euclid(count);
    show_frame(state, next as usize);
    Ok(())
}

fn frame_goto(state: &mut AppState, params: &Params) -> CommandResult {
    let frame = index(params, "frame").unwrap_or(0);
    if frame >= sprite(state)?.frames().len() {
        return Err(failed(SpriteError::UnknownFrame(frame)));
    }
    show_frame(state, frame);
    Ok(())
}

fn frame_move(state: &mut AppState, params: &Params) -> CommandResult {
    let (from, to) = (
        index(params, "from").unwrap_or(usize::MAX),
        index(params, "to").unwrap_or(usize::MAX),
    );
    transact(state, "Move Frame", |tx| tx.move_frame(from, to))?;
    state.cel_range = None;
    show_frame(state, to);
    Ok(())
}

fn frame_set_duration(state: &mut AppState, params: &Params) -> CommandResult {
    let (first, last) = match index(params, "frame") {
        Some(frame) => (frame, frame),
        None => target_frames(state)?,
    };
    let Some(ms) = params.int("ms") else {
        let frames = sprite(state)?.frames();
        let ms = frames.get(first).map_or(100, |frame| frame.duration_ms);
        state.dialog = Some(Dialog::FrameDuration { ms });
        return Ok(());
    };
    let ms = u32::try_from(ms.max(1)).unwrap_or(u32::MAX);
    transact(state, "Frame Duration", |tx| {
        (first..=last).try_for_each(|frame| tx.set_frame_duration(frame, ms))
    })
}

/// Selects a cel in the timeline: its layer and frame become the active ones.
fn timeline_select(state: &mut AppState, params: &Params) -> CommandResult {
    let document = document(state)?;
    let layer = match params.int("layer") {
        Some(id) => LayerId(u32::try_from(id).unwrap_or(u32::MAX)),
        None => document.active_layer(),
    };
    let frame = index(params, "frame").unwrap_or(document.active_frame());
    if document.sprite().layer(layer).is_none() {
        return Err(failed(SpriteError::UnknownLayer(layer.0)));
    }
    if frame >= document.sprite().frames().len() {
        return Err(failed(SpriteError::UnknownFrame(frame)));
    }
    // Extending keeps the frame the selection began on, as long as it stays on one layer.
    let anchor = match state.cel_range {
        Some(range) if params.bool("extend") == Some(true) && range.layer == layer => range.anchor,
        _ => frame,
    };
    state.cel_range = Some(CelRange {
        layer,
        anchor,
        extent: frame,
    });
    show_frame(state, frame);
    if let Some(document) = state.editor.document_mut() {
        document.set_active_layer(layer);
    }
    Ok(())
}

fn cel_move(state: &mut AppState, params: &Params) -> CommandResult {
    let layer = |name| LayerId(u32::try_from(params.int(name).unwrap_or(-1)).unwrap_or(u32::MAX));
    let frame = |name| index(params, name).unwrap_or(usize::MAX);
    let from = (layer("layer"), frame("frame"));
    let to = (layer("to_layer"), frame("to_frame"));
    let copy = params.bool("copy") == Some(true);
    let name = if copy { "Copy Cel" } else { "Move Cel" };
    transact(state, name, |tx| tx.move_cel(from, to, copy))?;
    state.cel_range = Some(CelRange {
        layer: to.0,
        anchor: to.1,
        extent: to.1,
    });
    show_frame(state, to.1);
    if let Some(document) = state.editor.document_mut() {
        document.set_active_layer(to.0);
    }
    Ok(())
}

fn cel_link(state: &mut AppState, _: &Params) -> CommandResult {
    let range = state
        .cel_range
        .filter(|range| range.first() != range.last());
    let Some(range) = range else {
        return Err(CommandError::failed(
            "Select the cels to link first: click one in the timeline, then Shift+click \
             another on the same layer.",
        ));
    };
    let others: Vec<usize> = (range.first() + 1..=range.last()).collect();
    transact(state, "Link Cels", |tx| {
        tx.link_cels(range.layer, range.first(), &others)
    })
}

fn cel_unlink(state: &mut AppState, _: &Params) -> CommandResult {
    let document = document(state)?;
    let (layer, frames) = match state.cel_range {
        Some(range) => (range.layer, range.first()..=range.last()),
        None => {
            let frame = document.active_frame();
            (document.active_layer(), frame..=frame)
        }
    };
    transact(state, "Unlink Cels", |tx| {
        frames
            .clone()
            .try_for_each(|frame| tx.unlink_cel(layer, frame))
    })
}

/// The tag a command names with its `tag` argument.
fn tag_at(state: &AppState, params: &Params) -> Result<(usize, Tag), CommandError> {
    let at = index(params, "tag").unwrap_or(usize::MAX);
    let tag = sprite(state)?.tags().get(at).cloned();
    tag.map(|tag| (at, tag))
        .ok_or_else(|| failed(SpriteError::UnknownTag(at)))
}

/// `base` with whatever the arguments say about a tag put over it.
fn tag_from(params: &Params, base: Tag) -> Result<Tag, CommandError> {
    let direction = match params.text("direction") {
        None => base.direction,
        Some(id) => Direction::from_id(id).ok_or_else(|| {
            CommandError::failed(format!(
                "\"{id}\" is not a direction. They are: forward, reverse, pingpong."
            ))
        })?,
    };
    let name = params.text("name").map_or(base.name, str::to_owned);
    if name.trim().is_empty() {
        return Err(CommandError::failed("A tag needs a name."));
    }
    Ok(Tag {
        name: name.trim().to_owned(),
        from: index(params, "from").unwrap_or(base.from),
        to: index(params, "to").unwrap_or(base.to),
        direction,
        color: params.color("color").unwrap_or(base.color),
    })
}

fn tag_new(state: &mut AppState, params: &Params) -> CommandResult {
    let (first, last) = target_frames(state)?;
    let count = sprite(state)?.tags().len();
    let rgb = TAG_COLORS[count % TAG_COLORS.len()];
    let base = Tag {
        name: format!("Tag {}", count + 1),
        from: first,
        to: last,
        direction: Direction::Forward,
        color: Rgba8::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
    };
    if params.text("name").is_none() {
        state.dialog = Some(Dialog::Tag {
            index: None,
            name: base.name,
            from: base.from,
            to: base.to,
            direction: base.direction,
            color: base.color,
        });
        return Ok(());
    }
    let tag = tag_from(params, base)?;
    transact(state, "New Tag", |tx| tx.add_tag(tag))?;
    Ok(())
}

fn tag_edit(state: &mut AppState, params: &Params) -> CommandResult {
    let (at, tag) = tag_at(state, params)?;
    if params.text("name").is_none() {
        state.dialog = Some(Dialog::Tag {
            index: Some(at),
            name: tag.name,
            from: tag.from,
            to: tag.to,
            direction: tag.direction,
            color: tag.color,
        });
        return Ok(());
    }
    let tag = tag_from(params, tag)?;
    transact(state, "Edit Tag", |tx| tx.set_tag(at, tag))
}

fn tag_delete(state: &mut AppState, params: &Params) -> CommandResult {
    let (at, _) = tag_at(state, params)?;
    transact(state, "Delete Tag", |tx| tx.remove_tag(at))?;
    // The tags after it moved up one place.
    state.play_tag = match state.play_tag {
        Some(playing) if playing == at => None,
        Some(playing) if playing > at => Some(playing - 1),
        other => other,
    };
    Ok(())
}

fn playback_set_tag(state: &mut AppState, params: &Params) -> CommandResult {
    state.play_tag = match params.int("tag") {
        Some(at) if at >= 0 => Some(tag_at(state, params)?.0),
        _ => None,
    };
    Ok(())
}

fn onion_set(state: &mut AppState, params: &Params) -> CommandResult {
    let frames = |name, current: u8| match params.int(name) {
        Some(count) => count.clamp(0, i64::from(MAX_ONION_FRAMES)) as u8,
        None => current,
    };
    let onion = &mut state.onion;
    onion.before = frames("before", onion.before);
    onion.after = frames("after", onion.after);
    if let Some(opacity) = params.int("opacity") {
        onion.opacity = opacity.clamp(0, 255) as u8;
    }
    onion.tint = params.bool("tint").unwrap_or(onion.tint);
    Ok(())
}

/// The frames an export writes: one pass through the tag named by `tag`, or every frame.
fn export_frames(state: &AppState, params: &Params) -> Result<Vec<usize>, CommandError> {
    let tag = match params.int("tag") {
        Some(at) if at >= 0 => Some(tag_at(state, params)?.0),
        _ => None,
    };
    Ok(playback::sequence(sprite(state)?, tag))
}

fn could_not_export(path: &Path, err: impl std::fmt::Display) -> CommandError {
    CommandError::failed(format!("Could not export {}: {err}", path.display()))
}

/// What the three exports do before they have a path: without any setting they open the
/// dialog, with settings they ask for the file. Returns the path once there is one.
fn export_path<'a>(
    state: &mut AppState,
    params: &'a Params,
    kind: ExportKind,
    command: &'static str,
    file_type: (&'static str, &'static str),
) -> Option<&'a Path> {
    if let Some(path) = params.text("path") {
        return Some(Path::new(path));
    }
    if params.is_empty() {
        state.dialog = Some(Dialog::Export {
            kind,
            scale: 1,
            tag: state.play_tag,
            sheet: SheetOptions::default(),
            json: true,
        });
    } else {
        state.file_request = Some(FileRequest::Export {
            command,
            params: params.clone(),
            file_type,
        });
    }
    None
}

fn scale_of(params: &Params) -> u32 {
    u32::try_from(params.int("scale").unwrap_or(1)).unwrap_or(0)
}

fn export_gif(state: &mut AppState, params: &Params) -> CommandResult {
    let kind = ExportKind::Gif;
    let file_type = ("GIF animation", "gif");
    let Some(path) = export_path(state, params, kind, "file.export_gif", file_type) else {
        return Ok(());
    };
    let frames = export_frames(state, params)?;
    export_gif_to_path(sprite(state)?, &frames, scale_of(params), path)
        .map_err(|err| could_not_export(path, err))?;
    state.log.push(format!("Exported {}", path.display()));
    Ok(())
}

fn export_sequence(state: &mut AppState, params: &Params) -> CommandResult {
    let kind = ExportKind::Sequence;
    let file_type = ("PNG image", "png");
    let Some(path) = export_path(state, params, kind, "file.export_sequence", file_type) else {
        return Ok(());
    };
    let frames = export_frames(state, params)?;
    let files = export_png_sequence(sprite(state)?, &frames, scale_of(params), path)
        .map_err(|err| could_not_export(path, err))?;
    if let (Some(first), Some(last)) = (files.first(), files.last()) {
        let name = |file: &Path| {
            file.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        };
        state.log.push(format!(
            "Exported {} files, {} to {}, to {}",
            files.len(),
            name(first),
            name(last),
            first.parent().unwrap_or(Path::new(".")).display()
        ));
    }
    Ok(())
}

fn export_sheet_command(state: &mut AppState, params: &Params) -> CommandResult {
    let kind = ExportKind::Sheet;
    let file_type = ("PNG image", "png");
    let Some(path) = export_path(state, params, kind, "file.export_sheet", file_type) else {
        return Ok(());
    };
    let layout = match params.text("layout") {
        None => SheetLayout::Rows,
        Some(id) => SheetLayout::from_id(id).ok_or_else(|| {
            CommandError::failed(format!(
                "\"{id}\" is not a sheet layout. They are: rows, columns, packed."
            ))
        })?,
    };
    let count = |name| u32::try_from(params.int(name).unwrap_or(0).max(0)).unwrap_or(u32::MAX);
    let options = SheetOptions {
        layout,
        per_line: count("per_line"),
        padding: count("padding"),
        trim: params.bool("trim") == Some(true),
    };
    let with_json = params.bool("json") != Some(false);
    let frames = export_frames(state, params)?;
    let sheet = export_sheet(sprite(state)?, &frames, &options, path, with_json)
        .map_err(|err| could_not_export(path, err))?;
    state.log.push(format!(
        "Exported {} ({} × {}, {} frames{})",
        path.display(),
        sheet.image.width(),
        sheet.image.height(),
        sheet.frames.len(),
        if with_json { ", with .json" } else { "" }
    ));
    Ok(())
}

pub fn register(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Color, Int, Text};

    // Frames.
    registry.register(
        Command::new("frame.new", "New Frame", |state: &mut AppState, _| {
            frame_new(state, false)
        })
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "frame.duplicate",
            "Duplicate Frame",
            |state: &mut AppState, _| frame_new(state, true),
        )
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("frame.delete", "Delete Frame", frame_delete).enabled_when(has_frames),
    )?;
    let steps: [(&str, &str, isize); 2] = [
        ("frame.next", "Next Frame", 1),
        ("frame.previous", "Previous Frame", -1),
    ];
    for (id, name, step) in steps {
        registry.register(
            Command::new(id, name, move |state: &mut AppState, _| {
                frame_step(state, step)
            })
            .enabled_when(has_frames),
        )?;
    }
    registry.register(
        Command::new("frame.first", "First Frame", |state: &mut AppState, _| {
            show_frame(state, 0);
            Ok(())
        })
        .enabled_when(has_frames),
    )?;
    registry.register(
        Command::new("frame.last", "Last Frame", |state: &mut AppState, _| {
            let last = sprite(state)?.frames().len() - 1;
            show_frame(state, last);
            Ok(())
        })
        .enabled_when(has_frames),
    )?;
    registry.register(
        Command::new("frame.goto", "Go to Frame", frame_goto)
            .param("frame", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("frame.move", "Move Frame", frame_move)
            .param("from", Int)
            .param("to", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("frame.set_duration", "Frame Duration…", frame_set_duration)
            .optional_param("ms", Int)
            .optional_param("frame", Int)
            .enabled_when(has_document),
    )?;

    // Cels.
    registry.register(
        Command::new("timeline.select", "Select Cel", timeline_select)
            .optional_param("layer", Int)
            .optional_param("frame", Int)
            .optional_param("extend", Bool)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("cel.move", "Move Cel", cel_move)
            .param("layer", Int)
            .param("frame", Int)
            .param("to_layer", Int)
            .param("to_frame", Int)
            .optional_param("copy", Bool)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("cel.link", "Link Selected Cels", cel_link).enabled_when(has_frames),
    )?;
    registry
        .register(Command::new("cel.unlink", "Unlink Cels", cel_unlink).enabled_when(has_frames))?;

    // Tags.
    registry.register(
        Command::new("tag.new", "New Tag…", tag_new)
            .optional_param("name", Text)
            .optional_param("from", Int)
            .optional_param("to", Int)
            .optional_param("direction", Text)
            .optional_param("color", Color)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("tag.edit", "Edit Tag", tag_edit)
            .param("tag", Int)
            .optional_param("name", Text)
            .optional_param("from", Int)
            .optional_param("to", Int)
            .optional_param("direction", Text)
            .optional_param("color", Color)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("tag.delete", "Delete Tag", tag_delete)
            .param("tag", Int)
            .enabled_when(has_document),
    )?;

    // Playback and onion skin.
    registry.register(
        Command::new(
            "playback.toggle",
            "Play / Stop",
            |state: &mut AppState, _| {
                state.toggle_playback();
                Ok(())
            },
        )
        .enabled_when(has_frames),
    )?;
    registry.register(
        Command::new("playback.set_tag", "Play Tag", playback_set_tag)
            .optional_param("tag", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "onion.toggle",
            "Toggle Onion Skin",
            |state: &mut AppState, params| {
                state.onion_enabled = params.bool("on").unwrap_or(!state.onion_enabled);
                Ok(())
            },
        )
        .optional_param("on", Bool),
    )?;
    registry.register(
        Command::new("onion.set", "Set Onion Skin", onion_set)
            .optional_param("before", Int)
            .optional_param("after", Int)
            .optional_param("opacity", Int)
            .optional_param("tint", Bool),
    )?;
    registry.register(
        Command::new(
            "preview.set",
            "Set Preview",
            |state: &mut AppState, params| {
                if let Some(scale) = params.int("scale") {
                    state.preview.scale = scale.clamp(1, i64::from(MAX_PREVIEW_SCALE)) as u32;
                }
                state.preview.tiled = params.bool("tiled").unwrap_or(state.preview.tiled);
                Ok(())
            },
        )
        .optional_param("scale", Int)
        .optional_param("tiled", Bool),
    )?;

    // Exports.
    registry.register(
        Command::new("file.export_gif", "Export GIF…", export_gif)
            .optional_param("path", Text)
            .optional_param("scale", Int)
            .optional_param("tag", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "file.export_sheet",
            "Export Spritesheet…",
            export_sheet_command,
        )
        .optional_param("path", Text)
        .optional_param("layout", Text)
        .optional_param("per_line", Int)
        .optional_param("padding", Int)
        .optional_param("trim", Bool)
        .optional_param("json", Bool)
        .optional_param("tag", Int)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "file.export_sequence",
            "Export PNG Sequence…",
            export_sequence,
        )
        .optional_param("path", Text)
        .optional_param("scale", Int)
        .optional_param("tag", Int)
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

    const L1: LayerId = LayerId(1);

    fn text(file: &TempFile) -> String {
        file.path().to_string_lossy().into_owned()
    }

    fn history(f: &Fixture) -> Vec<String> {
        f.document().history().map(str::to_owned).collect()
    }

    fn message(f: &mut Fixture) -> String {
        match f.state.dialog.take() {
            Some(Dialog::Message { text, .. }) => text,
            other => panic!("expected a message, found {other:?}"),
        }
    }

    fn frame(f: &Fixture) -> usize {
        f.document().active_frame()
    }

    fn frames(f: &Fixture) -> usize {
        f.document().sprite().frames().len()
    }

    /// Paints one red pixel at `x` on the frame being shown.
    fn mark(f: &mut Fixture, x: i32) {
        let frame = frame(f);
        let dot = Image::from_pixels(1, 1, vec![RED]).unwrap();
        transact(&mut f.state, "Mark", |tx| {
            tx.set_pixels(L1, frame, Rect::new(x, 0, 1, 1), &dot)
        })
        .unwrap();
    }

    /// Where the mark of each frame is.
    fn marks(f: &Fixture) -> Vec<Option<i32>> {
        let sprite = f.document().sprite();
        (0..sprite.frames().len())
            .map(|frame| sprite.cel(L1, frame).map(|cel| cel.origin().x))
            .collect()
    }

    fn select(f: &mut Fixture, frame: i64, extend: bool) {
        let params = Params::new()
            .with("layer", 1_u32)
            .with("frame", frame)
            .with("extend", extend);
        f.run_with("timeline.select", params);
    }

    /// A sprite with four frames, marked at x = 0, 1, 2 and 3, showing the first.
    fn four_frames() -> Fixture {
        let mut f = Fixture::with_sprite();
        mark(&mut f, 0);
        for x in 1..4 {
            f.run("frame.new");
            mark(&mut f, x);
        }
        f.run("frame.first");
        f
    }

    #[test]
    fn frames_are_added_after_the_one_shown_and_shown_in_turn() {
        let mut f = Fixture::with_sprite();
        assert!(!f.is_enabled("frame.delete") && !f.is_enabled("frame.next"));
        mark(&mut f, 5);
        f.run("frame.new");
        assert_eq!((frames(&f), frame(&f)), (2, 1));
        assert_eq!(marks(&f), [Some(5), None]);
        f.run("frame.first");
        f.run("frame.duplicate");
        assert_eq!(frame(&f), 1);
        assert_eq!(marks(&f), [Some(5), Some(5), None]);
        assert_eq!(history(&f), ["Mark", "New Frame", "Duplicate Frame"]);

        // Stepping wraps around at both ends.
        f.run("frame.next");
        f.run("frame.next");
        assert_eq!(frame(&f), 0);
        f.run("frame.previous");
        assert_eq!(frame(&f), 2);
        f.run("frame.first");
        assert_eq!(frame(&f), 0);
        f.run("frame.last");
        assert_eq!(frame(&f), 2);
        f.run_with("frame.goto", Params::new().with("frame", 1_i64));
        assert_eq!(frame(&f), 1);
        f.run_with("frame.goto", Params::new().with("frame", 9_i64));
        assert!(message(&mut f).contains("no frame"));
        // Going from frame to frame is not something to undo.
        assert_eq!(history(&f).len(), 3);

        f.run("edit.undo");
        assert_eq!((frames(&f), frame(&f)), (2, 1));
    }

    #[test]
    fn frames_are_deleted_one_or_several_but_never_all() {
        let mut f = four_frames();
        f.run("frame.next");
        f.run("frame.delete");
        assert_eq!(marks(&f), [Some(0), Some(2), Some(3)]);
        assert_eq!(frame(&f), 1);

        // The frames selected in the timeline go together, as one step.
        select(&mut f, 1, false);
        select(&mut f, 2, true);
        f.run("frame.delete");
        assert_eq!(marks(&f), [Some(0)]);
        assert_eq!(frame(&f), 0);
        assert_eq!(f.state.cel_range, None);
        f.run("edit.undo");
        assert_eq!(marks(&f), [Some(0), Some(2), Some(3)]);

        select(&mut f, 0, false);
        select(&mut f, 2, true);
        f.run("frame.delete");
        assert!(message(&mut f).contains("at least one frame"));
        assert_eq!(frames(&f), 3);
    }

    #[test]
    fn frames_move_and_durations_are_set() {
        let mut f = four_frames();
        let places = |from: i64, to: i64| Params::new().with("from", from).with("to", to);
        f.run_with("frame.move", places(0, 2));
        assert_eq!(marks(&f), [Some(1), Some(2), Some(0), Some(3)]);
        assert_eq!(frame(&f), 2);
        f.run_with("frame.move", places(0, 9));
        assert!(message(&mut f).contains("no frame"));

        let durations = |f: &Fixture| -> Vec<u32> {
            let frames = f.document().sprite().frames().iter();
            frames.map(|frame| frame.duration_ms).collect()
        };
        f.run("frame.set_duration");
        assert_eq!(
            f.state.dialog.take(),
            Some(Dialog::FrameDuration { ms: 100 })
        );
        f.run_with("frame.set_duration", Params::new().with("ms", 40_i64));
        assert_eq!(durations(&f), [100, 100, 40, 100]);
        let one = Params::new().with("ms", 250_i64).with("frame", 0_i64);
        f.run_with("frame.set_duration", one);
        assert_eq!(durations(&f), [250, 100, 40, 100]);
        // With several frames selected, all of them, as one step.
        select(&mut f, 1, false);
        select(&mut f, 3, true);
        let steps = history(&f).len();
        f.run_with("frame.set_duration", Params::new().with("ms", 60_i64));
        assert_eq!(durations(&f), [250, 60, 60, 60]);
        assert_eq!(history(&f).len(), steps + 1);
        f.run_with("frame.set_duration", Params::new().with("ms", -5_i64));
        assert_eq!(durations(&f)[1], 1);
    }

    #[test]
    fn selecting_in_the_timeline_picks_the_layer_and_frame() {
        let mut f = four_frames();
        f.run("layer.new");
        let upper = f.document().active_layer();
        select(&mut f, 2, false);
        assert_eq!((f.document().active_layer(), frame(&f)), (L1, 2));
        let range = f.state.cel_range.unwrap();
        assert_eq!((range.layer, range.first(), range.last()), (L1, 2, 2));

        // Extending keeps where it began, in either direction.
        select(&mut f, 0, true);
        let range = f.state.cel_range.unwrap();
        assert_eq!((range.anchor, range.first(), range.last()), (2, 0, 2));
        assert_eq!(frame(&f), 0);
        select(&mut f, 3, true);
        let range = f.state.cel_range.unwrap();
        assert_eq!((range.first(), range.last()), (2, 3));

        // On another layer it starts afresh.
        let other = Params::new()
            .with("layer", upper.0)
            .with("frame", 1_i64)
            .with("extend", true);
        f.run_with("timeline.select", other);
        let range = f.state.cel_range.unwrap();
        assert_eq!((range.layer, range.first(), range.last()), (upper, 1, 1));

        f.run_with("timeline.select", Params::new().with("frame", 12_i64));
        assert!(message(&mut f).contains("no frame"));
        f.run_with("timeline.select", Params::new().with("layer", 99_u32));
        assert!(message(&mut f).contains("no layer"));
        assert!(history(&f).iter().all(|name| name != "Select Cel"));
    }

    #[test]
    fn cels_are_moved_copied_linked_and_unlinked() {
        let mut f = four_frames();
        let cel = |frame: i64, to: i64, copy: bool| {
            Params::new()
                .with("layer", 1_u32)
                .with("frame", frame)
                .with("to_layer", 1_u32)
                .with("to_frame", to)
                .with("copy", copy)
        };
        f.run_with("cel.move", cel(0, 3, false));
        assert_eq!(marks(&f), [None, Some(1), Some(2), Some(0)]);
        assert_eq!(frame(&f), 3);
        f.run_with("cel.move", cel(1, 0, true));
        assert_eq!(marks(&f), [Some(1), Some(1), Some(2), Some(0)]);
        assert_eq!(
            history(&f)[history(&f).len() - 2..],
            ["Move Cel", "Copy Cel"]
        );
        f.run_with("cel.move", cel(1, 8, false));
        assert!(message(&mut f).contains("no frame"));

        // Linking needs several cels selected.
        select(&mut f, 2, false);
        f.run("cel.link");
        assert!(message(&mut f).contains("Shift+click"));
        select(&mut f, 3, true);
        f.run("cel.link");
        assert_eq!(marks(&f), [Some(1), Some(1), Some(2), Some(2)]);
        assert_eq!(f.document().sprite().linked_frames(L1, 3), [2, 3]);
        // Drawing on one shows on the other.
        f.run_with("frame.goto", Params::new().with("frame", 3_i64));
        f.draw_line(6);
        assert_eq!(f.document().sprite().pixel(L1, 2, 5, 6), RED);
        assert_eq!(f.document().sprite().pixel(L1, 1, 5, 6), Rgba8::TRANSPARENT);

        f.run("cel.unlink");
        assert!(f.document().sprite().link_groups().is_empty());
        f.draw_line(9);
        assert_eq!(f.document().sprite().pixel(L1, 2, 5, 9), Rgba8::TRANSPARENT);
        assert_eq!(
            history(&f)[history(&f).len() - 4..],
            ["Link Cels", "Pencil", "Unlink Cels", "Pencil"]
        );
    }

    #[test]
    fn tags_are_made_changed_and_deleted() {
        let mut f = four_frames();
        select(&mut f, 1, false);
        select(&mut f, 2, true);
        // Without a name the dialog opens, filled in from the selected frames.
        f.run("tag.new");
        let Some(Dialog::Tag {
            index: None,
            name,
            from: 1,
            to: 2,
            ..
        }) = f.state.dialog.take()
        else {
            panic!("expected the tag dialog");
        };
        assert_eq!(name, "Tag 1");

        let walk = Params::new()
            .with("name", "Walk")
            .with("from", 1_i64)
            .with("to", 3_i64)
            .with("direction", "pingpong");
        f.run_with("tag.new", walk);
        f.run_with("tag.new", Params::new().with("name", "Here"));
        let tags = |f: &Fixture| -> Vec<(String, usize, usize, Direction)> {
            let tags = f.document().sprite().tags().iter();
            tags.map(|t| (t.name.clone(), t.from, t.to, t.direction))
                .collect()
        };
        assert_eq!(
            tags(&f),
            [
                ("Walk".to_owned(), 1, 3, Direction::PingPong),
                ("Here".to_owned(), 1, 2, Direction::Forward)
            ]
        );
        // Each new tag gets the next colour.
        let colors: Vec<Rgba8> = f
            .document()
            .sprite()
            .tags()
            .iter()
            .map(|t| t.color)
            .collect();
        assert_ne!(colors[0], colors[1]);

        f.run_with("tag.edit", Params::new().with("tag", 1_i64));
        assert!(matches!(
            f.state.dialog.take(),
            Some(Dialog::Tag { index: Some(1), .. })
        ));
        let renamed = Params::new()
            .with("tag", 1_i64)
            .with("name", "  Idle ")
            .with("to", 1_i64)
            .with("direction", "reverse");
        f.run_with("tag.edit", renamed);
        assert_eq!(tags(&f)[1], ("Idle".to_owned(), 1, 1, Direction::Reverse));

        for (params, complaint) in [
            (Params::new().with("name", " "), "needs a name"),
            (Params::new().with("name", "x").with("to", 9_i64), "frame"),
            (
                Params::new().with("name", "x").with("direction", "up"),
                "not a direction",
            ),
        ] {
            f.run_with("tag.new", params);
            assert!(message(&mut f).contains(complaint));
        }
        f.run_with("tag.edit", Params::new().with("tag", 5_i64));
        assert!(message(&mut f).contains("no tag"));

        // Deleting a tag keeps the playing tag pointing at the same tag.
        f.run_with("playback.set_tag", Params::new().with("tag", 1_i64));
        f.run_with("tag.delete", Params::new().with("tag", 0_i64));
        assert_eq!(f.state.play_tag, Some(0));
        f.run_with("tag.delete", Params::new().with("tag", 0_i64));
        assert_eq!(f.state.play_tag, None);
        assert!(tags(&f).is_empty());
        f.run("edit.undo");
        f.run("edit.undo");
        assert_eq!(tags(&f).len(), 2);
    }

    #[test]
    fn playing_moves_through_the_frames_and_stopping_goes_back() {
        let mut f = four_frames();
        f.run_with("frame.goto", Params::new().with("frame", 1_i64));
        f.run("playback.toggle");
        assert!(f.state.playing.is_some());
        // The first tick only starts the clock.
        assert_eq!(f.state.tick_playback(10.0), Some(100));
        assert_eq!(frame(&f), 1);
        assert_eq!(f.state.tick_playback(10.0625), Some(38));
        assert_eq!(frame(&f), 1);
        f.state.tick_playback(10.125);
        assert_eq!(frame(&f), 2);
        f.state.tick_playback(10.375);
        assert_eq!(frame(&f), 0);
        // Playing is not editing.
        assert!(history(&f).iter().all(|name| name != "Play / Stop"));
        let steps = history(&f).len();

        f.run("playback.toggle");
        assert!(f.state.playing.is_none());
        assert_eq!(frame(&f), 1);
        assert_eq!(f.state.tick_playback(11.0), None);
        assert_eq!(history(&f).len(), steps);

        // Picking a frame by hand stops it where the user put it.
        f.run("playback.toggle");
        f.state.tick_playback(20.0);
        f.run("frame.last");
        assert!(f.state.playing.is_none());
        assert_eq!(frame(&f), 3);
    }

    #[test]
    fn a_tag_can_be_looped_and_a_stroke_holds_the_frame() {
        use acms_core::geom::Point;
        use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

        let mut f = four_frames();
        let tag = Params::new()
            .with("name", "Pair")
            .with("from", 2_i64)
            .with("to", 3_i64);
        f.run_with("tag.new", tag);
        f.run_with("playback.set_tag", Params::new().with("tag", 0_i64));
        assert_eq!(f.state.play_tag, Some(0));
        f.run_with("frame.goto", Params::new().with("frame", 2_i64));
        f.run("playback.toggle");
        f.state.tick_playback(0.0);
        let mut seen = Vec::new();
        for step in 1..=5 {
            // Just past each tenth of a second, when the next frame is due.
            f.state.tick_playback(f64::from(step) * 0.1 + 0.001);
            seen.push(frame(&f));
        }
        assert_eq!(seen, [3, 2, 3, 2, 3]);

        // While a stroke is being drawn, time stands still.
        f.state.editor.pointer_down(PointerEvent {
            position: Point::new(4, 4),
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        });
        f.state.tick_playback(0.95);
        assert_eq!(frame(&f), 3);
        assert!(f.state.editor.is_interacting());

        f.run_with("playback.set_tag", Params::new().with("tag", -1_i64));
        assert_eq!(f.state.play_tag, None);
        f.run_with("playback.set_tag", Params::new().with("tag", 7_i64));
        assert!(message(&mut f).contains("no tag"));
    }

    #[test]
    fn onion_skin_and_preview_settings() {
        let mut f = Fixture::new();
        f.run("onion.toggle");
        assert!(f.state.onion_enabled);
        f.run_with("onion.toggle", Params::new().with("on", false));
        assert!(!f.state.onion_enabled);
        let settings = Params::new()
            .with("before", 3_i64)
            .with("after", 99_i64)
            .with("opacity", 300_i64)
            .with("tint", false);
        f.run_with("onion.set", settings);
        let onion = f.state.onion;
        assert_eq!((onion.before, onion.after), (3, MAX_ONION_FRAMES));
        assert_eq!((onion.opacity, onion.tint), (255, false));
        f.run_with("onion.set", Params::new().with("before", 0_i64));
        assert_eq!(
            (f.state.onion.before, f.state.onion.after),
            (0, MAX_ONION_FRAMES)
        );

        f.run_with("preview.set", Params::new().with("scale", 4_i64));
        f.run_with("preview.set", Params::new().with("tiled", true));
        assert_eq!((f.state.preview.scale, f.state.preview.tiled), (4, true));
        f.run_with("preview.set", Params::new().with("scale", 400_i64));
        assert_eq!(f.state.preview.scale, MAX_PREVIEW_SCALE);
    }

    #[test]
    fn exports_ask_for_settings_then_a_file_then_write_it() {
        let mut f = four_frames();
        let walk = Params::new()
            .with("name", "Walk")
            .with("from", 1_i64)
            .with("to", 3_i64)
            .with("direction", "pingpong");
        f.run_with("tag.new", walk);

        for (command, kind) in [
            ("file.export_gif", ExportKind::Gif),
            ("file.export_sheet", ExportKind::Sheet),
            ("file.export_sequence", ExportKind::Sequence),
        ] {
            f.run(command);
            assert!(
                matches!(f.state.dialog.take(), Some(Dialog::Export { kind: k, .. }) if k == kind)
            );
            // With settings but no path, the file dialog is next.
            let settings = Params::new().with("tag", 0_i64);
            f.run_with(command, settings.clone());
            let Some(FileRequest::Export {
                command: asked,
                params,
                ..
            }) = f.state.file_request.take()
            else {
                panic!("expected a file request for {command}");
            };
            assert_eq!((asked, params), (command, settings));
        }

        // A GIF of the tag: there and back.
        let gif = TempFile::path_only("anim.gif");
        let params = Params::new()
            .with("path", text(&gif))
            .with("tag", 0_i64)
            .with("scale", 2_i64);
        f.run_with("file.export_gif", params);
        let bytes = std::fs::read(gif.path()).unwrap();
        let mut decoder = gif::DecodeOptions::new().read_info(&bytes[..]).unwrap();
        assert_eq!((decoder.width(), decoder.height()), (32, 32));
        let mut count = 0;
        while decoder.read_next_frame().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 4);

        // A sheet with its metadata.
        let png = TempFile::path_only("sheet.png");
        let json = TempFile::path_only("sheet.json");
        let params = Params::new()
            .with("path", text(&png))
            .with("layout", "columns")
            .with("per_line", 2_i64)
            .with("padding", 1_i64);
        f.run_with("file.export_sheet", params);
        let sheet = acms_io::png::decode_png(&std::fs::read(png.path()).unwrap()).unwrap();
        assert_eq!((sheet.width(), sheet.height()), (1 + 17 * 2, 1 + 17 * 2));
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(json.path()).unwrap()).unwrap();
        assert_eq!(meta["frames"].as_object().unwrap().len(), 4);
        assert_eq!(meta["meta"]["frameTags"][0]["name"], "Walk");
        assert!(f.state.log.last().unwrap().contains("4 frames, with .json"));

        // A PNG sequence of every frame.
        let first = TempFile::path_only("frames_1.png");
        let target = TempFile::path_only("frames.png");
        f.run_with(
            "file.export_sequence",
            Params::new().with("path", text(&target)),
        );
        assert!(first.path().exists() && !target.path().exists());
        for number in 2..=4 {
            let name = first
                .path()
                .to_string_lossy()
                .replace("_1.png", &format!("_{number}.png"));
            std::fs::remove_file(name).unwrap();
        }

        // Failures are said out loud, and nothing here is an undo step.
        let bad = Params::new()
            .with("path", text(&png))
            .with("layout", "spiral");
        f.run_with("file.export_sheet", bad);
        assert!(message(&mut f).contains("not a sheet layout"));
        let bad = Params::new().with("path", text(&gif)).with("scale", 0_i64);
        f.run_with("file.export_gif", bad);
        assert!(message(&mut f).contains("Could not export"));
        let bad = Params::new().with("path", text(&gif)).with("tag", 9_i64);
        f.run_with("file.export_gif", bad);
        assert!(message(&mut f).contains("no tag"));
        assert!(history(&f).iter().all(|name| !name.starts_with("Export")));
    }

    #[test]
    fn an_animation_survives_saving_and_opening() {
        let mut f = four_frames();
        select(&mut f, 0, false);
        select(&mut f, 1, true);
        f.run("cel.link");
        f.run_with("frame.set_duration", Params::new().with("ms", 70_i64));
        f.run_with("tag.new", Params::new().with("name", "Start"));
        let saved = TempFile::path_only("animation.acms");
        f.run_with("file.save_as", Params::new().with("path", text(&saved)));
        let sprite = f.document().sprite().clone();
        f.run("file.close");
        f.run_with("file.open", Params::new().with("path", text(&saved)));
        assert_eq!(*f.document().sprite(), sprite);
        assert_eq!(f.document().sprite().tags()[0].name, "Start");
        assert_eq!(f.document().sprite().link_groups(), [(L1, vec![0, 1])]);
    }
}
