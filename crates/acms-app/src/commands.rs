//! The app's commands. Menus, shortcuts, the palette, panels and dialogs all go through
//! [`run`]; nothing else changes the document.

use std::path::{Path, PathBuf};

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::document::Document;
use acms_core::editor::Editor;
use acms_core::sprite::Sprite;
use acms_core::stroke::BrushShape;
use acms_io::acms::{load_acms, save_acms};
use acms_io::png::export_png_to_path;
use egui::Vec2;

use crate::panels;
use crate::state::{AppState, Dialog, FileRequest, PendingCommand};

type CommandResult = Result<(), CommandError>;

/// Size offered by the New Sprite dialog.
const DEFAULT_SPRITE_SIZE: u32 = 32;

/// Runs a command and reports a failure where the user will see it: every error goes to the
/// log, and a command that ran but failed (a file that would not save, say) also opens a
/// message.
pub fn run(registry: &CommandRegistry<AppState>, state: &mut AppState, id: &str, params: &Params) {
    let Err(err) = registry.run(id, state, params) else {
        return;
    };
    state.log.push(err.to_string());
    if let CommandError::Failed(reason) = err {
        let title = registry
            .get(id)
            .map_or(id, |command| command.name())
            .trim_end_matches('…')
            .to_owned();
        state.dialog = Some(Dialog::Message {
            title,
            text: reason,
        });
    }
}

/// Turns an error message written as a fragment ("the layer is locked") into a sentence for
/// a dialog ("The layer is locked.").
pub fn sentence(message: impl std::fmt::Display) -> String {
    let message = message.to_string();
    let mut chars = message.chars();
    let mut sentence: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return message,
    };
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

fn has_document(state: &AppState) -> bool {
    state.editor.document().is_some()
}

fn no_document() -> CommandError {
    CommandError::failed("No sprite is open.")
}

/// If the open sprite has unsaved changes, asks the user before `id` throws them away and
/// returns `true`: the command should stop, and runs again with `discard` set if they agree.
fn must_confirm_discard(state: &mut AppState, id: &str, params: &Params) -> bool {
    if !state.has_unsaved_changes() || params.bool("discard") == Some(true) {
        return false;
    }
    state.dialog = Some(Dialog::ConfirmDiscard {
        then: PendingCommand {
            id: id.to_owned(),
            params: params.clone().with("discard", true),
        },
    });
    true
}

fn file_new(state: &mut AppState, params: &Params) -> CommandResult {
    let (Some(width), Some(height)) = (params.int("width"), params.int("height")) else {
        state.dialog = Some(Dialog::NewSprite {
            width: DEFAULT_SPRITE_SIZE,
            height: DEFAULT_SPRITE_SIZE,
        });
        return Ok(());
    };
    if must_confirm_discard(state, "file.new", params) {
        return Ok(());
    }
    // Sizes that do not even fit in a `u32` are reported like any other bad size.
    let side = |n: i64| u32::try_from(n).unwrap_or(0);
    let sprite = Sprite::new(side(width), side(height))
        .map_err(|err| CommandError::failed(sentence(err)))?;
    state.open_document(Document::new(sprite), None);
    Ok(())
}

fn file_open(state: &mut AppState, params: &Params) -> CommandResult {
    if must_confirm_discard(state, "file.open", params) {
        return Ok(());
    }
    let Some(path) = params.text("path").map(PathBuf::from) else {
        state.file_request = Some(FileRequest::Open { discard: true });
        return Ok(());
    };
    let sprite = load_acms(&path)
        .map_err(|err| CommandError::failed(format!("Could not open {}: {err}", path.display())))?;
    state.log.push(format!("Opened {}", path.display()));
    state.open_document(Document::new(sprite), Some(path));
    Ok(())
}

fn save_to(state: &mut AppState, path: &Path) -> CommandResult {
    let document = state.editor.document().ok_or_else(no_document)?;
    save_acms(document.sprite(), path)
        .map_err(|err| CommandError::failed(format!("Could not save {}: {err}", path.display())))?;
    state.saved_revision = document.revision();
    state.file_path = Some(path.to_owned());
    state.log.push(format!("Saved {}", path.display()));
    Ok(())
}

fn file_save(state: &mut AppState, _: &Params) -> CommandResult {
    match state.file_path.clone() {
        Some(path) => save_to(state, &path),
        None => {
            state.file_request = Some(FileRequest::SaveAs);
            Ok(())
        }
    }
}

fn file_save_as(state: &mut AppState, params: &Params) -> CommandResult {
    match params.text("path") {
        Some(path) => save_to(state, Path::new(path)),
        None => {
            state.file_request = Some(FileRequest::SaveAs);
            Ok(())
        }
    }
}

fn file_export_png(state: &mut AppState, params: &Params) -> CommandResult {
    let scale = params.int("scale");
    let Some(path) = params.text("path").map(PathBuf::from) else {
        match scale {
            // The scale is known, only the file name is missing.
            Some(scale) => {
                let scale = u32::try_from(scale).unwrap_or(0);
                state.file_request = Some(FileRequest::ExportPng { scale });
            }
            None => state.dialog = Some(Dialog::ExportPng { scale: 1 }),
        }
        return Ok(());
    };

    let document = state.editor.document().ok_or_else(no_document)?;
    let scale = u32::try_from(scale.unwrap_or(1)).unwrap_or(0);
    export_png_to_path(document.sprite(), document.active_frame(), scale, &path).map_err(
        |err| CommandError::failed(format!("Could not export {}: {err}", path.display())),
    )?;
    state.log.push(format!("Exported {}", path.display()));
    Ok(())
}

fn brush_set_shape(state: &mut AppState, params: &Params) -> CommandResult {
    state.editor.brush.shape = match params.text("shape") {
        Some("round") => BrushShape::Round,
        Some("square") => BrushShape::Square,
        other => {
            return Err(CommandError::failed(format!(
                "The brush shape must be \"round\" or \"square\", not {other:?}."
            )));
        }
    };
    Ok(())
}

/// Zooms one level in (`+1`) or out (`-1`) around the anchor given in `params`, or around the
/// centre of the canvas.
fn zoom(state: &mut AppState, params: &Params, steps: i32) -> CommandResult {
    let anchor = Vec2::new(
        params.float("anchor_x").unwrap_or(0.0) as f32,
        params.float("anchor_y").unwrap_or(0.0) as f32,
    );
    state.view.zoom_by(steps, anchor);
    Ok(())
}

fn zoom_fit(state: &mut AppState, _: &Params) -> CommandResult {
    let sprite = state.editor.document().ok_or_else(no_document)?.sprite();
    let size = Vec2::new(sprite.width() as f32, sprite.height() as f32);
    state.view.fit(size);
    Ok(())
}

/// Registers every built-in command.
pub fn register_builtin(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Color, Float, Int, Text};

    // File
    registry.register(
        Command::new("file.new", "New Sprite…", file_new)
            .optional_param("width", Int)
            .optional_param("height", Int)
            .optional_param("discard", Bool),
    )?;
    registry.register(
        Command::new("file.open", "Open…", file_open)
            .optional_param("path", Text)
            .optional_param("discard", Bool),
    )?;
    registry.register(Command::new("file.save", "Save", file_save).enabled_when(has_document))?;
    registry.register(
        Command::new("file.save_as", "Save As…", file_save_as)
            .optional_param("path", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("file.export_png", "Export PNG…", file_export_png)
            .optional_param("path", Text)
            .optional_param("scale", Int)
            .enabled_when(has_document),
    )?;

    // Edit
    registry.register(
        Command::new("edit.undo", "Undo", |state: &mut AppState, _| {
            state.editor.document_mut().ok_or_else(no_document)?.undo();
            Ok(())
        })
        .enabled_when(|state| state.editor.document().is_some_and(Document::can_undo)),
    )?;
    registry.register(
        Command::new("edit.redo", "Redo", |state: &mut AppState, _| {
            state.editor.document_mut().ok_or_else(no_document)?.redo();
            Ok(())
        })
        .enabled_when(|state| state.editor.document().is_some_and(Document::can_redo)),
    )?;
    registry.register(
        Command::new(
            "edit.history_jump",
            "Jump to History Step",
            |state: &mut AppState, params| {
                let steps = usize::try_from(params.int("steps").unwrap_or(0)).unwrap_or(0);
                state
                    .editor
                    .document_mut()
                    .ok_or_else(no_document)?
                    .jump_to(steps);
                Ok(())
            },
        )
        .param("steps", Int)
        .enabled_when(has_document),
    )?;

    crate::layer_commands::register(registry)?;

    // Tools: one command per registered tool, so each can have its own shortcut.
    let tools: Vec<_> = Editor::new().tools().collect();
    for (tool, name) in tools {
        registry.register(Command::new(
            format!("tool.{tool}"),
            format!("Tool: {name}"),
            move |state: &mut AppState, _| {
                state.editor.set_active_tool(tool);
                Ok(())
            },
        ))?;
    }

    // Colour and brush
    registry.register(
        Command::new(
            "color.set_primary",
            "Set Primary Colour",
            |state: &mut AppState, params| {
                state.editor.primary = params.color("color").unwrap_or(state.editor.primary);
                Ok(())
            },
        )
        .param("color", Color),
    )?;
    registry.register(
        Command::new(
            "color.set_secondary",
            "Set Secondary Colour",
            |state: &mut AppState, params| {
                state.editor.secondary = params.color("color").unwrap_or(state.editor.secondary);
                Ok(())
            },
        )
        .param("color", Color),
    )?;
    registry.register(Command::new(
        "color.swap",
        "Swap Primary and Secondary Colour",
        |state: &mut AppState, _| {
            state.editor.swap_colors();
            Ok(())
        },
    ))?;
    registry.register(
        Command::new(
            "brush.set_size",
            "Set Brush Size",
            |state: &mut AppState, params| {
                let size = params
                    .int("size")
                    .unwrap_or(1)
                    .clamp(1, i64::from(u32::MAX));
                state.editor.brush.set_size(size as u32);
                Ok(())
            },
        )
        .param("size", Int),
    )?;
    registry.register(
        Command::new("brush.set_shape", "Set Brush Shape", brush_set_shape).param("shape", Text),
    )?;

    // View
    registry.register(
        Command::new("view.zoom_in", "Zoom In", |state: &mut AppState, params| {
            zoom(state, params, 1)
        })
        .optional_param("anchor_x", Float)
        .optional_param("anchor_y", Float)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "view.zoom_out",
            "Zoom Out",
            |state: &mut AppState, params| zoom(state, params, -1),
        )
        .optional_param("anchor_x", Float)
        .optional_param("anchor_y", Float)
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("view.zoom_fit", "Zoom to Fit", zoom_fit).enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "view.pan",
            "Pan the Canvas",
            |state: &mut AppState, params| {
                let dx = params.float("dx").unwrap_or(0.0) as f32;
                let dy = params.float("dy").unwrap_or(0.0) as f32;
                state.view.pan += Vec2::new(dx, dy);
                Ok(())
            },
        )
        .param("dx", Float)
        .param("dy", Float),
    )?;
    registry.register(Command::new(
        "view.toggle_grid",
        "Toggle Pixel Grid",
        |state: &mut AppState, _| {
            state.view.show_grid = !state.view.show_grid;
            Ok(())
        },
    ))?;
    registry.register(Command::new(
        "view.command_palette",
        "Command Palette",
        |state: &mut AppState, _| {
            state.palette.open();
            Ok(())
        },
    ))?;
    registry.register(Command::new(
        "view.toggle_theme",
        "Toggle Light / Dark Theme",
        |state: &mut AppState, _| {
            state.theme = state.theme.toggled();
            Ok(())
        },
    ))?;
    registry.register(Command::new(
        "view.reset_layout",
        "Reset Panel Layout",
        |state: &mut AppState, _| {
            state.dock = panels::default_layout();
            state.layout_generation += 1;
            Ok(())
        },
    ))?;
    registry.register(
        Command::new("view.clear_log", "Clear Log", |state: &mut AppState, _| {
            state.log.clear();
            Ok(())
        })
        .enabled_when(|state| !state.log.is_empty()),
    )?;

    // App
    registry.register(
        Command::new("app.quit", "Quit", |state: &mut AppState, params| {
            if !must_confirm_discard(state, "app.quit", params) {
                state.quit_requested = true;
            }
            Ok(())
        })
        .optional_param("discard", Bool),
    )?;
    Ok(())
}

/// A registry and state to run commands against in tests.
#[cfg(test)]
pub mod testing {
    use super::*;
    use acms_core::color::Rgba8;
    use acms_core::geom::Point;
    use acms_core::sprite::LayerId;
    use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

    pub const RED: Rgba8 = Rgba8::rgb(255, 0, 0);

    pub struct Fixture {
        pub registry: CommandRegistry<AppState>,
        pub state: AppState,
    }

    impl Fixture {
        pub fn new() -> Self {
            let mut registry = CommandRegistry::new();
            register_builtin(&mut registry).unwrap();
            Self {
                registry,
                state: AppState::default(),
            }
        }

        /// With a blank 16×16 sprite open.
        pub fn with_sprite() -> Self {
            let mut fixture = Self::new();
            fixture.run_with(
                "file.new",
                Params::new().with("width", 16_u32).with("height", 16_u32),
            );
            fixture
        }

        pub fn run(&mut self, id: &str) {
            self.run_with(id, Params::new());
        }

        pub fn run_with(&mut self, id: &str, params: Params) {
            run(&self.registry, &mut self.state, id, &params);
        }

        pub fn try_run(&mut self, id: &str, params: Params) -> Result<(), CommandError> {
            self.registry.run(id, &mut self.state, &params)
        }

        pub fn is_enabled(&self, id: &str) -> bool {
            self.registry
                .get(id)
                .is_some_and(|command| command.is_enabled(&self.state))
        }

        pub fn document(&self) -> &Document {
            self.state.editor.document().unwrap()
        }

        /// Draws a horizontal red line on row `y` of the selected layer, as one stroke.
        pub fn draw_line(&mut self, y: i32) {
            let event = |x| PointerEvent {
                position: Point::new(x, y),
                button: PointerButton::Primary,
                modifiers: Modifiers::default(),
            };
            self.state.editor.primary = RED;
            self.state.editor.pointer_down(event(2));
            self.state.editor.pointer_move(event(10));
            self.state.editor.pointer_up(event(10));
        }

        /// A pixel of the bottom layer the sprite was created with.
        pub fn pixel(&self, x: i32, y: i32) -> Rgba8 {
            self.pixel_on(LayerId(1), x, y)
        }

        pub fn pixel_on(&self, layer: LayerId, x: i32, y: i32) -> Rgba8 {
            self.document().sprite().pixel(layer, 0, x, y)
        }

        pub fn confirm_pending(&mut self) {
            let Some(Dialog::ConfirmDiscard { then }) = self.state.dialog.take() else {
                panic!("expected a confirmation, found {:?}", self.state.dialog);
            };
            self.run_with(&then.id, then.params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Fixture, RED};
    use super::*;
    use crate::config::TempFile;
    use crate::keymap;
    use crate::panels::Panel;
    use crate::theme::ThemeKind;
    use acms_core::color::Rgba8;
    use acms_core::geom::Point;
    use acms_core::tool::{Modifiers, PointerButton, PointerEvent};

    #[test]
    fn every_default_shortcut_points_at_a_registered_command() {
        let fixture = Fixture::new();
        for command in keymap::default_commands() {
            assert!(
                fixture.registry.get(command).is_some(),
                "no command `{command}`"
            );
        }
    }

    #[test]
    fn new_without_a_size_opens_the_dialog_and_with_a_size_creates_the_sprite() {
        let mut fixture = Fixture::new();
        fixture.run("file.new");
        assert_eq!(
            fixture.state.dialog,
            Some(Dialog::NewSprite {
                width: 32,
                height: 32
            })
        );
        assert!(fixture.state.editor.document().is_none());

        fixture.state.dialog = None;
        fixture.run_with(
            "file.new",
            Params::new().with("width", 48_u32).with("height", 20_u32),
        );
        let sprite = fixture.state.editor.document().unwrap().sprite();
        assert_eq!((sprite.width(), sprite.height()), (48, 20));
        assert!(fixture.state.view.fit_pending);
        assert!(!fixture.state.has_unsaved_changes());
        assert_eq!(fixture.state.dialog, None);
    }

    #[test]
    fn an_impossible_size_is_reported_and_changes_nothing() {
        let mut fixture = Fixture::new();
        for (width, height) in [(0_i64, 16_i64), (16, 100_000), (-5, 16), (i64::MAX, 16)] {
            let params = Params::new().with("width", width).with("height", height);
            fixture.run_with("file.new", params);
            assert!(fixture.state.editor.document().is_none());
            assert!(matches!(
                fixture.state.dialog.take(),
                Some(Dialog::Message { .. })
            ));
        }
        assert_eq!(fixture.state.log.len(), 4);
    }

    #[test]
    fn undo_and_redo_follow_the_strokes() {
        let mut fixture = Fixture::with_sprite();
        assert!(matches!(
            fixture.try_run("edit.undo", Params::new()),
            Err(CommandError::Disabled(_))
        ));

        fixture.draw_line(3);
        fixture.draw_line(6);
        assert_eq!(fixture.pixel(5, 6), RED);

        fixture.run("edit.undo");
        assert_eq!(fixture.pixel(5, 6), Rgba8::TRANSPARENT);
        assert_eq!(fixture.pixel(5, 3), RED);
        fixture.run("edit.redo");
        assert_eq!(fixture.pixel(5, 6), RED);
        assert!(matches!(
            fixture.try_run("edit.redo", Params::new()),
            Err(CommandError::Disabled(_))
        ));

        fixture.run_with("edit.history_jump", Params::new().with("steps", 0_i64));
        assert_eq!(fixture.pixel(5, 3), Rgba8::TRANSPARENT);
        fixture.run_with("edit.history_jump", Params::new().with("steps", 2_i64));
        assert_eq!(fixture.pixel(5, 6), RED);
    }

    #[test]
    fn undo_abandons_a_stroke_that_is_still_being_drawn() {
        let mut fixture = Fixture::with_sprite();
        fixture.draw_line(3);
        fixture.state.editor.pointer_down(PointerEvent {
            position: Point::new(1, 9),
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        });
        fixture.run("edit.undo");
        assert!(!fixture.state.editor.is_interacting());
        assert_eq!(
            fixture
                .state
                .editor
                .document()
                .unwrap()
                .sprite()
                .cels()
                .count(),
            0
        );
    }

    #[test]
    fn tool_colour_and_brush_commands() {
        let mut fixture = Fixture::new();
        fixture.run("tool.eraser");
        assert_eq!(fixture.state.editor.active_tool(), "eraser");
        fixture.run("tool.eyedropper");
        assert_eq!(fixture.state.editor.active_tool(), "eyedropper");
        fixture.run("tool.pencil");
        assert_eq!(fixture.state.editor.active_tool(), "pencil");

        fixture.run_with("color.set_primary", Params::new().with("color", RED));
        // Text that is a hex colour works too, as it would from a keymap file.
        fixture.run_with(
            "color.set_secondary",
            Params::new().with("color", "#00FF0080"),
        );
        assert_eq!(fixture.state.editor.primary, RED);
        assert_eq!(fixture.state.editor.secondary, Rgba8::new(0, 255, 0, 128));
        fixture.run("color.swap");
        assert_eq!(fixture.state.editor.secondary, RED);

        fixture.run_with("brush.set_size", Params::new().with("size", 7_i64));
        assert_eq!(fixture.state.editor.brush.size(), 7);
        fixture.run_with("brush.set_size", Params::new().with("size", -3_i64));
        assert_eq!(fixture.state.editor.brush.size(), 1);
        fixture.run_with("brush.set_size", Params::new().with("size", 100_000_i64));
        assert_eq!(fixture.state.editor.brush.size(), 64);

        fixture.run_with("brush.set_shape", Params::new().with("shape", "square"));
        assert_eq!(fixture.state.editor.brush.shape, BrushShape::Square);
        fixture.run_with("brush.set_shape", Params::new().with("shape", "round"));
        assert_eq!(fixture.state.editor.brush.shape, BrushShape::Round);
        assert!(
            fixture
                .try_run("brush.set_shape", Params::new().with("shape", "star"))
                .is_err()
        );
        assert!(fixture.try_run("brush.set_size", Params::new()).is_err());
    }

    #[test]
    fn view_commands() {
        let mut fixture = Fixture::with_sprite();
        fixture.state.view.viewport = Vec2::new(848.0, 648.0);
        fixture.run("view.zoom_fit");
        assert_eq!(fixture.state.view.zoom(), 32.0);

        fixture.run("view.zoom_out");
        assert_eq!(fixture.state.view.zoom(), 24.0);
        fixture.run("view.zoom_in");
        fixture.run("view.zoom_in");
        assert_eq!(fixture.state.view.zoom(), 48.0);

        fixture.run_with("view.pan", Params::new().with("dx", 10.0).with("dy", -4.0));
        assert_eq!(fixture.state.view.pan, Vec2::new(10.0, -4.0));

        assert!(fixture.state.view.show_grid);
        fixture.run("view.toggle_grid");
        assert!(!fixture.state.view.show_grid);

        fixture.run("view.toggle_theme");
        assert_eq!(fixture.state.theme, ThemeKind::Light);

        // Zooming needs a sprite.
        let mut empty = Fixture::new();
        assert!(matches!(
            empty.try_run("view.zoom_in", Params::new()),
            Err(CommandError::Disabled(_))
        ));
    }

    #[test]
    fn reset_layout_brings_closed_panels_back() {
        let mut fixture = Fixture::new();
        let before = fixture.state.dock.iter_all_tabs().count();
        fixture.state.dock.retain_tabs(|tab| *tab != Panel::Log);
        assert_eq!(fixture.state.dock.iter_all_tabs().count(), before - 1);

        let generation = fixture.state.layout_generation;
        fixture.run("view.reset_layout");
        assert_eq!(fixture.state.dock.iter_all_tabs().count(), before);
        assert_ne!(fixture.state.layout_generation, generation);
    }

    #[test]
    fn clear_log_is_only_enabled_when_there_is_something_to_clear() {
        let mut fixture = Fixture::new();
        assert!(matches!(
            fixture.try_run("view.clear_log", Params::new()),
            Err(CommandError::Disabled(_))
        ));
        fixture.state.log.push("something".to_owned());
        fixture.run("view.clear_log");
        assert!(fixture.state.log.is_empty());
    }

    #[test]
    fn save_reopen_and_export_round_trip_through_real_files() {
        let acms = TempFile::path_only("commands-roundtrip.acms");
        let png = TempFile::path_only("commands-export.png");
        let path = |file: &TempFile| file.path().to_string_lossy().into_owned();

        let mut fixture = Fixture::with_sprite();
        fixture.draw_line(4);
        assert!(fixture.state.has_unsaved_changes());

        // Save without a file name asks for one; with a name it writes the file.
        fixture.run("file.save");
        assert_eq!(fixture.state.file_request.take(), Some(FileRequest::SaveAs));
        fixture.run_with("file.save_as", Params::new().with("path", path(&acms)));
        assert!(!fixture.state.has_unsaved_changes());
        assert_eq!(fixture.state.file_path.as_deref(), Some(acms.path()));
        let saved = fixture.state.editor.document().unwrap().sprite().clone();

        // Further edits are unsaved; a plain Save now goes to the same file.
        fixture.draw_line(8);
        assert!(fixture.state.has_unsaved_changes());
        fixture.run("file.save");
        assert!(!fixture.state.has_unsaved_changes());
        assert_eq!(fixture.state.file_request, None);
        let saved_again = fixture.state.editor.document().unwrap().sprite().clone();
        assert_ne!(saved, saved_again);

        // Undoing back to what is on disk is "no unsaved changes" again.
        fixture.draw_line(12);
        fixture.run("edit.undo");
        assert!(!fixture.state.has_unsaved_changes());

        // Export, with a scale.
        fixture.run_with(
            "file.export_png",
            Params::new().with("path", path(&png)).with("scale", 3_i64),
        );
        let exported = acms_io::png::decode_png(&std::fs::read(png.path()).unwrap()).unwrap();
        assert_eq!((exported.width(), exported.height()), (48, 48));
        assert_eq!(exported.get(5 * 3 + 1, 8 * 3 + 1), RED);
        assert_eq!(exported.get(1, 1), Rgba8::TRANSPARENT);

        // Reopen in a fresh app: the same sprite comes back, with its path and no history.
        let mut reopened = Fixture::new();
        reopened.run_with("file.open", Params::new().with("path", path(&acms)));
        let document = reopened.state.editor.document().unwrap();
        assert_eq!(*document.sprite(), saved_again);
        assert!(!document.can_undo());
        assert_eq!(reopened.state.file_path.as_deref(), Some(acms.path()));
        assert!(!reopened.state.has_unsaved_changes());
        assert_eq!(reopened.state.dialog, None);
    }

    #[test]
    fn open_and_export_without_a_path_ask_for_one() {
        let mut fixture = Fixture::with_sprite();
        fixture.run("file.open");
        assert_eq!(
            fixture.state.file_request.take(),
            Some(FileRequest::Open { discard: true })
        );

        fixture.run("file.export_png");
        assert_eq!(
            fixture.state.dialog.take(),
            Some(Dialog::ExportPng { scale: 1 })
        );
        fixture.run_with("file.export_png", Params::new().with("scale", 4_i64));
        assert_eq!(
            fixture.state.file_request.take(),
            Some(FileRequest::ExportPng { scale: 4 })
        );

        // Saving and exporting need a sprite.
        let mut empty = Fixture::new();
        for id in ["file.save", "file.save_as", "file.export_png"] {
            assert!(matches!(
                empty.try_run(id, Params::new()),
                Err(CommandError::Disabled(_))
            ));
        }
    }

    #[test]
    fn failed_file_operations_show_a_message_and_keep_the_sprite() {
        let mut fixture = Fixture::with_sprite();
        fixture.draw_line(4);
        let before = fixture.state.editor.document().unwrap().sprite().clone();

        let missing = TempFile::path_only("commands-missing.acms");
        let path = missing.path().to_string_lossy().into_owned();
        fixture.run_with(
            "file.open",
            Params::new().with("path", path).with("discard", true),
        );
        let Some(Dialog::Message { title, text }) = fixture.state.dialog.take() else {
            panic!("expected a message");
        };
        assert_eq!(title, "Open");
        assert!(text.starts_with("Could not open "));
        assert_eq!(*fixture.state.editor.document().unwrap().sprite(), before);
        assert!(fixture.state.has_unsaved_changes());

        // A file that is not a sprite.
        let garbage = TempFile::new("commands-garbage.acms", "this is not a zip archive");
        let path = garbage.path().to_string_lossy().into_owned();
        fixture.run_with(
            "file.open",
            Params::new().with("path", path).with("discard", true),
        );
        assert!(matches!(
            fixture.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
        assert_eq!(*fixture.state.editor.document().unwrap().sprite(), before);

        // Saving into a folder that does not exist.
        let nowhere = std::env::temp_dir()
            .join("acms-test-no-such-folder")
            .join("x.acms");
        fixture.run_with(
            "file.save_as",
            Params::new().with("path", nowhere.to_string_lossy().into_owned()),
        );
        assert!(matches!(
            fixture.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
        assert!(fixture.state.has_unsaved_changes());
        assert_eq!(fixture.state.file_path, None);

        // An export scale that makes no sense.
        let png = TempFile::path_only("commands-bad-scale.png");
        fixture.run_with(
            "file.export_png",
            Params::new()
                .with("path", png.path().to_string_lossy().into_owned())
                .with("scale", 0_i64),
        );
        assert!(matches!(
            fixture.state.dialog.take(),
            Some(Dialog::Message { .. })
        ));
        assert!(!png.path().exists());
    }

    #[test]
    fn unsaved_changes_are_never_thrown_away_without_asking() {
        let mut fixture = Fixture::with_sprite();
        fixture.draw_line(4);
        let drawn = fixture.state.editor.document().unwrap().sprite().clone();
        let new_8x8 = || Params::new().with("width", 8_u32).with("height", 8_u32);

        // New, Open and Quit all stop and ask.
        fixture.run_with("file.new", new_8x8());
        assert!(matches!(
            fixture.state.dialog,
            Some(Dialog::ConfirmDiscard { .. })
        ));
        assert_eq!(*fixture.state.editor.document().unwrap().sprite(), drawn);

        fixture.state.dialog = None;
        fixture.run("file.open");
        assert!(matches!(
            fixture.state.dialog,
            Some(Dialog::ConfirmDiscard { .. })
        ));
        assert_eq!(fixture.state.file_request, None);

        fixture.state.dialog = None;
        fixture.run("app.quit");
        assert!(matches!(
            fixture.state.dialog,
            Some(Dialog::ConfirmDiscard { .. })
        ));
        assert!(!fixture.state.quit_requested);

        // Cancelling (closing the dialog) leaves everything as it was.
        fixture.state.dialog = None;
        assert_eq!(*fixture.state.editor.document().unwrap().sprite(), drawn);

        // Agreeing runs the original command.
        fixture.run_with("file.new", new_8x8());
        fixture.confirm_pending();
        assert_eq!(fixture.state.editor.document().unwrap().sprite().width(), 8);
        assert_eq!(fixture.state.dialog, None);

        fixture.draw_line(2);
        fixture.run("file.open");
        fixture.confirm_pending();
        assert_eq!(
            fixture.state.file_request.take(),
            Some(FileRequest::Open { discard: true })
        );

        fixture.run("app.quit");
        fixture.confirm_pending();
        assert!(fixture.state.quit_requested);
    }

    #[test]
    fn nothing_is_asked_when_there_is_nothing_to_lose() {
        let mut fixture = Fixture::with_sprite();
        fixture.run_with(
            "file.new",
            Params::new().with("width", 8_u32).with("height", 8_u32),
        );
        assert_eq!(fixture.state.dialog, None);
        assert_eq!(fixture.state.editor.document().unwrap().sprite().width(), 8);

        fixture.run("app.quit");
        assert!(fixture.state.quit_requested);
    }

    #[test]
    fn commands_reject_arguments_they_do_not_declare() {
        let mut fixture = Fixture::new();
        fixture.run_with("view.toggle_grid", Params::new().with("loud", true));
        assert!(fixture.state.view.show_grid);
        assert_eq!(
            fixture.state.log,
            ["command `view.toggle_grid`: there is no parameter `loud`"]
        );
        assert_eq!(fixture.state.dialog, None);
    }
}
