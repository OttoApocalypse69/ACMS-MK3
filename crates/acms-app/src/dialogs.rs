//! Modal dialogs and native file dialogs.
//!
//! A dialog collects what a command needs and then runs that command; it changes nothing else.

use std::path::PathBuf;

use acms_core::color::Rgba8;
use acms_core::command::{CommandRegistry, Params};
use acms_core::geom::{Point, Rect};
use acms_core::palette::MAX_RAMP_STEPS;
use acms_core::sprite::{Direction, MAX_FRAME_MS, MAX_SPRITE_SIZE};
use acms_io::export::{SheetLayout, SheetOptions};
use acms_io::png::MAX_EXPORT_SCALE;
use egui::{DragValue, Grid, Id, Modal, RichText, Ui};

use crate::commands;
use crate::keymap::{self, Keymap};
use crate::settings::Preferences;
use crate::state::{Anchor, AppState, Dialog, ExportKind, FileRequest, SizePurpose};
use crate::theme::ThemeKind;

/// What the user did with a dialog this frame.
enum Outcome {
    /// Nothing yet; keep showing it.
    Open(Dialog),
    Dismissed,
    Run(String, Params),
    /// A script's dialog was answered: with its values, or `None` for Cancel.
    Script(Option<acms_script::DialogSpec>),
    /// Run a command and keep the dialog up, as it is given here.
    RunAndStay(String, Params, Dialog),
    /// Keep the dialog up even though Escape was pressed: the key was meant for something
    /// inside it.
    Stay(Dialog),
}

/// Shows the open dialog, if any.
pub fn show(
    ctx: &egui::Context,
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    keymap: &Keymap,
) {
    let Some(dialog) = state.dialog.take() else {
        return;
    };

    let modal = Modal::new(Id::new("dialog")).show(ctx, |ui| {
        ui.set_min_width(300.0);
        match dialog {
            Dialog::NewSprite { width, height } => new_sprite(ui, width, height),
            Dialog::ExportPng { scale } => export_png(ui, state, scale),
            Dialog::ConfirmDiscard { then } => confirm_discard(ui, state, then),
            Dialog::Message { title, text } => message(ui, title, text),
            Dialog::Size {
                purpose,
                width,
                height,
                anchor,
            } => size(ui, purpose, width, height, anchor),
            Dialog::Ramp { steps } => ramp(ui, state, steps),
            Dialog::Tag {
                index,
                name,
                from,
                to,
                direction,
                color,
            } => tag(ui, state, index, name, from, to, direction, color),
            Dialog::FrameDuration { ms } => frame_duration(ui, ms),
            Dialog::Script(spec) => script(ui, spec),
            Dialog::Preferences(draft) => preferences(ui, draft),
            Dialog::Keymap {
                filter,
                capture,
                note,
            } => keymap_editor(ui, state, registry, keymap, filter, capture, note),
            Dialog::Recover => recover(ui, state),
            Dialog::Timelapse { scale, step_ms } => timelapse(ui, state, scale, step_ms),
            Dialog::Slice {
                name,
                rect,
                center,
                pivot,
            } => slice(ui, name, rect, center, pivot),
            Dialog::Export {
                kind,
                scale,
                tag,
                sheet,
                json,
            } => export(ui, state, kind, scale, tag, sheet, json),
        }
    });

    // Clicking outside the dialog or pressing Escape is the same as Cancel.
    let dismissed = modal.should_close();
    match modal.inner {
        // The script behind a dialog is told when its dialog goes away.
        Outcome::Open(Dialog::Script(_)) if dismissed => {
            crate::scripting::answer_dialog(registry, state, None);
        }
        Outcome::Script(answer) => crate::scripting::answer_dialog(registry, state, answer),
        Outcome::Stay(dialog) => state.dialog = Some(dialog),
        Outcome::RunAndStay(id, params, dialog) => {
            commands::run(registry, state, &id, &params);
            // If the command had something to say, that is shown instead.
            if state.dialog.is_none() {
                state.dialog = Some(dialog);
            }
        }
        Outcome::Open(_) if dismissed => {}
        Outcome::Open(dialog) => state.dialog = Some(dialog),
        Outcome::Dismissed => {}
        Outcome::Run(id, params) => commands::run(registry, state, &id, &params),
    }
}

/// The row of buttons at the bottom of a dialog. Returns whether `confirm` or Cancel was
/// clicked.
fn buttons(ui: &mut Ui, confirm: &str) -> (bool, bool) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let confirmed = ui.button(confirm).clicked();
        let cancelled = ui.button("Cancel").clicked();
        (confirmed, cancelled)
    })
    .inner
}

fn new_sprite(ui: &mut Ui, mut width: u32, mut height: u32) -> Outcome {
    ui.heading("New Sprite");
    Grid::new("new_sprite").num_columns(2).show(ui, |ui| {
        for (label, value) in [("Width", &mut width), ("Height", &mut height)] {
            ui.label(label);
            ui.add(
                DragValue::new(value)
                    .range(1..=MAX_SPRITE_SIZE)
                    .suffix(" px"),
            );
            ui.end_row();
        }
    });

    match buttons(ui, "Create") {
        (true, _) => {
            let params = Params::new().with("width", width).with("height", height);
            Outcome::Run("file.new".to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::NewSprite { width, height }),
    }
}

fn export_png(ui: &mut Ui, state: &AppState, mut scale: u32) -> Outcome {
    ui.heading("Export PNG");
    Grid::new("export_png").num_columns(2).show(ui, |ui| {
        ui.label("Scale");
        ui.add(
            DragValue::new(&mut scale)
                .range(1..=MAX_EXPORT_SCALE)
                .suffix("×"),
        );
        ui.end_row();

        if let Some(document) = state.editor.document() {
            let sprite = document.sprite();
            ui.label("Size");
            ui.label(format!(
                "{} × {} px",
                sprite.width() * scale,
                sprite.height() * scale
            ));
            ui.end_row();
        }
    });

    match buttons(ui, "Export…") {
        (true, _) => Outcome::Run(
            "file.export_png".to_owned(),
            Params::new().with("scale", scale),
        ),
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::ExportPng { scale }),
    }
}

fn confirm_discard(ui: &mut Ui, state: &AppState, then: crate::state::PendingCommand) -> Outcome {
    ui.heading("Discard unsaved changes?");
    let name = state.document_title();
    let name = name.trim_end_matches(" •");
    ui.label(format!(
        "“{name}” has changes that have not been saved. They will be lost."
    ));

    match buttons(ui, "Discard Changes") {
        (true, _) => Outcome::Run(then.id, then.params),
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::ConfirmDiscard { then }),
    }
}

fn message(ui: &mut Ui, title: String, text: String) -> Outcome {
    ui.heading(&title);
    ui.label(RichText::new(&text));
    ui.add_space(8.0);
    if ui.button("OK").clicked() {
        Outcome::Dismissed
    } else {
        Outcome::Open(Dialog::Message { title, text })
    }
}

/// Width and height for a canvas resize, a sprite resize or scaling the selection.
fn size(
    ui: &mut Ui,
    purpose: SizePurpose,
    mut width: u32,
    mut height: u32,
    mut anchor: Anchor,
) -> Outcome {
    let (title, command, confirm) = match purpose {
        SizePurpose::CanvasSize => ("Canvas Size", "sprite.canvas_size", "Resize"),
        SizePurpose::SpriteSize => ("Sprite Size", "sprite.size", "Scale"),
        SizePurpose::ScaleSelection => ("Scale Selection", "selection.scale", "Scale"),
        SizePurpose::NewTilemap => (
            "New Tilemap Layer: Tile Size",
            "layer.new_tilemap",
            "Create",
        ),
    };
    ui.heading(title);
    Grid::new("size").num_columns(2).show(ui, |ui| {
        for (label, value) in [("Width", &mut width), ("Height", &mut height)] {
            ui.label(label);
            ui.add(
                DragValue::new(value)
                    .range(1..=MAX_SPRITE_SIZE)
                    .suffix(" px"),
            );
            ui.end_row();
        }
        if purpose == SizePurpose::CanvasSize {
            ui.label("Keep the picture at");
            egui::ComboBox::from_id_salt("anchor")
                .selected_text(anchor.id())
                .show_ui(ui, |ui| {
                    for choice in Anchor::ALL {
                        ui.selectable_value(&mut anchor, choice, choice.id());
                    }
                });
            ui.end_row();
        }
    });
    match purpose {
        SizePurpose::CanvasSize => {}
        SizePurpose::NewTilemap => {
            let hint = "The layer gets a tileset of its own. Draw on it to make tiles.";
            ui.label(RichText::new(hint).weak());
        }
        SizePurpose::SpriteSize | SizePurpose::ScaleSelection => {
            ui.label(RichText::new("Pixels are repeated or dropped, never blended.").weak());
        }
    }

    match buttons(ui, confirm) {
        (true, _) => {
            let mut params = Params::new().with("width", width).with("height", height);
            if purpose == SizePurpose::CanvasSize {
                params = params.with("anchor", anchor.id());
            }
            Outcome::Run(command.to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Size {
            purpose,
            width,
            height,
            anchor,
        }),
    }
}

/// A new tag, or an existing one being changed. Frames are shown counting from 1.
#[allow(clippy::too_many_arguments)]
fn tag(
    ui: &mut Ui,
    state: &AppState,
    index: Option<usize>,
    mut name: String,
    mut from: usize,
    mut to: usize,
    mut direction: Direction,
    mut color: Rgba8,
) -> Outcome {
    let frames = state
        .editor
        .document()
        .map_or(1, |document| document.sprite().frames().len());
    ui.heading(if index.is_some() {
        "Edit Tag"
    } else {
        "New Tag"
    });
    let mut delete = false;
    Grid::new("tag").num_columns(2).show(ui, |ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut name);
        ui.end_row();

        ui.label("Frames");
        ui.horizontal(|ui| {
            let (mut first, mut last) = (from + 1, to + 1);
            ui.add(DragValue::new(&mut first).range(1..=frames));
            ui.label("to");
            ui.add(DragValue::new(&mut last).range(1..=frames));
            from = first - 1;
            to = (last - 1).max(from);
        });
        ui.end_row();

        ui.label("Direction");
        egui::ComboBox::from_id_salt("tag_direction")
            .selected_text(direction.name())
            .show_ui(ui, |ui| {
                for choice in Direction::ALL {
                    ui.selectable_value(&mut direction, choice, choice.name());
                }
            });
        ui.end_row();

        ui.label("Colour");
        let mut rgba = color.to_array();
        if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
            color = Rgba8::from_array(rgba);
        }
        ui.end_row();
    });
    if index.is_some() {
        delete = ui.button("Delete Tag").clicked();
    }

    let confirm = if index.is_some() { "Change" } else { "Create" };
    match (buttons(ui, confirm), index) {
        _ if delete => Outcome::Run(
            "tag.delete".to_owned(),
            Params::new().with("tag", index.unwrap_or(0) as i64),
        ),
        ((true, _), index) => {
            let params = Params::new()
                .with("name", name.as_str())
                .with("from", from as i64)
                .with("to", to as i64)
                .with("direction", direction.id())
                .with("color", color);
            match index {
                Some(at) => Outcome::Run("tag.edit".to_owned(), params.with("tag", at as i64)),
                None => Outcome::Run("tag.new".to_owned(), params),
            }
        }
        ((_, true), _) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Tag {
            index,
            name,
            from,
            to,
            direction,
            color,
        }),
    }
}

fn preferences(ui: &mut Ui, mut draft: Preferences) -> Outcome {
    ui.heading("Preferences");
    Grid::new("preferences").num_columns(2).show(ui, |ui| {
        ui.label("Theme");
        egui::ComboBox::from_id_salt("preferences_theme")
            .selected_text(match draft.theme {
                ThemeKind::Dark => "Dark",
                ThemeKind::Light => "Light",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.theme, ThemeKind::Dark, "Dark");
                ui.selectable_value(&mut draft.theme, ThemeKind::Light, "Light");
            });
        ui.end_row();

        ui.label("Interface size")
            .on_hover_text("How large text and controls are drawn. The sprite is not affected.");
        let mut percent = (draft.ui_scale * 100.0).round();
        let range = Preferences::UI_SCALES;
        ui.add(
            DragValue::new(&mut percent)
                .range(range.start() * 100.0..=range.end() * 100.0)
                .speed(5.0)
                .suffix(" %"),
        );
        // Whole steps of 5 %, so that the number shown is the number used.
        draft.ui_scale = (percent / 5.0).round() * 5.0 / 100.0;
        ui.end_row();

        ui.label("Undo memory")
            .on_hover_text("How much memory the undo history of one sprite may use. The oldest steps are forgotten first.");
        ui.add(
            DragValue::new(&mut draft.history_mb)
                .range(Preferences::HISTORY_MB)
                .speed(8.0)
                .suffix(" MB"),
        );
        ui.end_row();

        ui.label("Autosave every")
            .on_hover_text("Sprites with unsaved changes are copied to a recovery folder, and offered back after a crash. 0 turns this off.");
        ui.add(
            DragValue::new(&mut draft.autosave_minutes)
                .range(Preferences::AUTOSAVE_MINUTES)
                .speed(0.2)
                .suffix(" min"),
        );
        ui.end_row();
    });

    match buttons(ui, "OK") {
        (true, _) => {
            let theme = match draft.theme {
                ThemeKind::Dark => "dark",
                ThemeKind::Light => "light",
            };
            let params = Params::new()
                .with("theme", theme)
                .with("ui_scale", f64::from(draft.ui_scale))
                .with("history_mb", draft.history_mb)
                .with("autosave_minutes", draft.autosave_minutes);
            Outcome::Run("prefs.set".to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Preferences(draft)),
    }
}

/// Every command that can be run by a key, with its keys and the buttons to change them.
fn keymap_editor(
    ui: &mut Ui,
    state: &AppState,
    registry: &CommandRegistry<AppState>,
    keymap: &Keymap,
    mut filter: String,
    capture: Option<String>,
    note: String,
) -> Outcome {
    ui.set_min_width(520.0);
    ui.heading("Keyboard Shortcuts");
    let name_of = |id: &str| {
        registry
            .get(id)
            .map_or_else(|| id.to_owned(), |command| command.name().to_owned())
    };

    // Waiting for the keys of one command: the next key press is the answer.
    if let Some(command) = capture {
        ui.label(format!("Press the new keys for \"{}\".", name_of(&command)));
        ui.label(RichText::new("Escape cancels.").weak());
        let pressed = ui.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some(egui::KeyboardShortcut::new(*modifiers, *key)),
                _ => None,
            })
        });
        return match pressed {
            Some(shortcut) if shortcut.logical_key == egui::Key::Escape => {
                Outcome::Stay(Dialog::Keymap {
                    filter,
                    capture: None,
                    note,
                })
            }
            Some(shortcut) => {
                let keys = keymap::format_chord(shortcut);
                // Read back the way the keymap will: Ctrl and Cmd are one modifier there.
                let previous = keymap::parse_chord(&keys)
                    .and_then(|shortcut| keymap.command_for(shortcut))
                    .filter(|previous| *previous != command);
                let note = match previous {
                    Some(previous) => format!(
                        "{keys} now runs \"{}\"; it was the key for \"{}\".",
                        name_of(&command),
                        name_of(previous)
                    ),
                    None => format!("{keys} now runs \"{}\".", name_of(&command)),
                };
                let params = Params::new()
                    .with("command", command.as_str())
                    .with("keys", keys.as_str());
                let dialog = Dialog::Keymap {
                    filter,
                    capture: None,
                    note,
                };
                Outcome::RunAndStay("keymap.bind".to_owned(), params, dialog)
            }
            None => Outcome::Open(Dialog::Keymap {
                filter,
                capture: Some(command),
                note,
            }),
        };
    }

    ui.horizontal(|ui| {
        ui.label("Find");
        ui.add(egui::TextEdit::singleline(&mut filter).hint_text("part of a command's name"));
    });
    if !note.is_empty() {
        ui.label(RichText::new(&note).weak());
    }
    let wanted = filter.to_lowercase();
    let mut action = None;
    let mut capture = None;
    egui::ScrollArea::vertical()
        .max_height(360.0)
        .auto_shrink(false)
        .show(ui, |ui| {
            Grid::new("keymap")
                .num_columns(5)
                .striped(true)
                .show(ui, |ui| {
                    for command in registry.iter().filter(|c| c.runs_without_params()) {
                        let id = command.id();
                        if !command.name().to_lowercase().contains(&wanted) {
                            continue;
                        }
                        let keys: Vec<String> = keymap
                            .shortcuts_for(id)
                            .map(|shortcut| ui.ctx().format_shortcut(shortcut))
                            .collect();
                        ui.label(command.name()).on_hover_text(id);
                        if keys.is_empty() {
                            ui.label(RichText::new("—").weak());
                        } else {
                            ui.label(keys.join(", "));
                        }
                        if ui
                            .small_button("Change…")
                            .on_hover_text("Press the keys that should run this command")
                            .clicked()
                        {
                            capture = Some(id.to_owned());
                        }
                        let clear = ui
                            .add_enabled(!keys.is_empty(), egui::Button::new("Clear").small())
                            .on_hover_text("Take its keys away");
                        if clear.clicked() {
                            action = Some(("keymap.clear", id.to_owned()));
                        }
                        if ui
                            .small_button("Reset")
                            .on_hover_text("Give it the keys it comes with")
                            .clicked()
                        {
                            action = Some(("keymap.reset", id.to_owned()));
                        }
                        ui.end_row();
                    }
                });
        });
    let _ = state;

    ui.add_space(8.0);
    let (reset_all, close) = ui
        .horizontal(|ui| {
            let reset_all = ui
                .button("Reset All")
                .on_hover_text("Every command gets the keys it comes with")
                .clicked();
            (reset_all, ui.button("Close").clicked())
        })
        .inner;

    let stay = |capture: Option<String>, note: String| Dialog::Keymap {
        filter: filter.clone(),
        capture,
        note,
    };
    if let Some((id, command)) = action {
        let params = Params::new().with("command", command.as_str());
        return Outcome::RunAndStay(id.to_owned(), params, stay(None, String::new()));
    }
    if reset_all {
        let note = "Every command has the keys it comes with.".to_owned();
        return Outcome::RunAndStay("keymap.reset".to_owned(), Params::new(), stay(None, note));
    }
    if close {
        return Outcome::Dismissed;
    }
    Outcome::Open(stay(capture, note))
}

/// Asks what to do with the sprites a run that did not end normally left behind.
fn recover(ui: &mut Ui, state: &AppState) -> Outcome {
    ui.heading("Recover Unsaved Sprites");
    let found = &state.recovery.found;
    ui.label(match found.len() {
        1 => "ACMS did not close properly last time. One sprite with unsaved changes was kept:"
            .to_owned(),
        count => format!(
            "ACMS did not close properly last time. {count} sprites with unsaved changes were kept:"
        ),
    });
    for leftover in found {
        let label = ui.label(format!("  •  {}", leftover.title()));
        if let Some(original) = &leftover.original {
            label.on_hover_text(original.display().to_string());
        }
    }
    ui.add_space(8.0);
    let (recover, discard, later) = ui
        .horizontal(|ui| {
            let recover = ui
                .button("Recover")
                .on_hover_text("Open them as they were when they were last autosaved")
                .clicked();
            let discard = ui
                .button("Discard")
                .on_hover_text("Delete the kept copies")
                .clicked();
            let later = ui
                .button("Later")
                .on_hover_text("Keep the copies and ask again next time")
                .clicked();
            (recover, discard, later)
        })
        .inner;
    if recover {
        Outcome::Run("file.recover".to_owned(), Params::new())
    } else if discard {
        Outcome::Run(
            "file.recover".to_owned(),
            Params::new().with("discard", true),
        )
    } else if later {
        Outcome::Dismissed
    } else {
        Outcome::Open(Dialog::Recover)
    }
}

fn timelapse(ui: &mut Ui, state: &AppState, mut scale: u32, mut step_ms: u32) -> Outcome {
    ui.heading("Export Timelapse");
    ui.label("How the sprite was made, replayed from the undo history as a GIF.");
    Grid::new("timelapse").num_columns(2).show(ui, |ui| {
        ui.label("Scale");
        ui.add(
            DragValue::new(&mut scale)
                .range(1..=MAX_EXPORT_SCALE)
                .suffix("×"),
        );
        ui.end_row();

        ui.label("Time per step");
        ui.add(
            DragValue::new(&mut step_ms)
                .range(10..=2000)
                .speed(5.0)
                .suffix(" ms"),
        );
        ui.end_row();

        if let Some(document) = state.editor.document() {
            ui.label("Steps");
            ui.label(document.history_cursor().to_string())
                .on_hover_text("Undo steps since the sprite was opened. A very long history is thinned out to 400 pictures.");
            ui.end_row();
        }
    });
    match buttons(ui, "Export…") {
        (true, _) => {
            let params = Params::new().with("scale", scale).with("step_ms", step_ms);
            Outcome::Run("file.export_timelapse".to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Timelapse { scale, step_ms }),
    }
}

/// A dialog a script built: one row per field.
fn script(ui: &mut Ui, mut spec: acms_script::DialogSpec) -> Outcome {
    use acms_script::FieldKind;
    ui.heading(&spec.title);
    Grid::new("script_dialog").num_columns(2).show(ui, |ui| {
        for field in &mut spec.fields {
            ui.label(&field.label);
            match &mut field.kind {
                FieldKind::Number {
                    value,
                    min,
                    max,
                    decimals,
                } => {
                    let places = usize::from(*decimals);
                    let speed = if places == 0 { 0.2 } else { 0.01 };
                    ui.add(
                        DragValue::new(value)
                            .range(*min..=*max)
                            .fixed_decimals(places)
                            .speed(speed),
                    );
                }
                FieldKind::Color(color) => {
                    let mut rgba = color.to_array();
                    if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                        *color = Rgba8::from_array(rgba);
                    }
                }
                FieldKind::Text(text) => {
                    ui.text_edit_singleline(text);
                }
                FieldKind::Check(on) => {
                    ui.checkbox(on, "");
                }
                FieldKind::Dropdown { options, selected } => {
                    let shown = options.get(*selected).map_or("", String::as_str);
                    egui::ComboBox::from_id_salt(("script_dropdown", field.id.as_str()))
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            for (index, option) in options.iter().enumerate() {
                                ui.selectable_value(selected, index, option.as_str());
                            }
                        });
                }
            }
            ui.end_row();
        }
    });
    match buttons(ui, "OK") {
        (true, _) => Outcome::Script(Some(spec)),
        (_, true) => Outcome::Script(None),
        _ => Outcome::Open(Dialog::Script(spec)),
    }
}

fn frame_duration(ui: &mut Ui, mut ms: u32) -> Outcome {
    ui.heading("Frame Duration");
    Grid::new("frame_duration").num_columns(2).show(ui, |ui| {
        ui.label("Shown for");
        ui.add(
            DragValue::new(&mut ms)
                .range(1..=MAX_FRAME_MS)
                .suffix(" ms"),
        );
        ui.end_row();
    });
    ui.label(RichText::new("Applies to the frames selected in the timeline.").weak());
    match buttons(ui, "Set") {
        (true, _) => Outcome::Run(
            "frame.set_duration".to_owned(),
            Params::new().with("ms", ms),
        ),
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::FrameDuration { ms }),
    }
}

/// The settings of a GIF, spritesheet or PNG sequence export.
fn export(
    ui: &mut Ui,
    state: &AppState,
    kind: ExportKind,
    mut scale: u32,
    mut tag: Option<usize>,
    mut sheet: SheetOptions,
    mut json: bool,
) -> Outcome {
    let (title, command) = match kind {
        ExportKind::Gif => ("Export GIF", "file.export_gif"),
        ExportKind::Sheet => ("Export Spritesheet", "file.export_sheet"),
        ExportKind::Sequence => ("Export PNG Sequence", "file.export_sequence"),
    };
    let tags: Vec<String> = state.editor.document().map_or_else(Vec::new, |document| {
        let tags = document.sprite().tags().iter();
        tags.map(|tag| tag.name.clone()).collect()
    });
    // The tag may have been deleted since the dialog was filled in.
    tag = tag.filter(|at| *at < tags.len());

    ui.heading(title);
    Grid::new("export").num_columns(2).show(ui, |ui| {
        ui.label("Frames");
        let shown = tag.map_or("All frames", |at| tags[at].as_str());
        egui::ComboBox::from_id_salt("export_tag")
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut tag, None, "All frames");
                for (at, name) in tags.iter().enumerate() {
                    ui.selectable_value(&mut tag, Some(at), name);
                }
            });
        ui.end_row();

        if kind == ExportKind::Sheet {
            ui.label("Layout");
            egui::ComboBox::from_id_salt("export_layout")
                .selected_text(sheet.layout.name())
                .show_ui(ui, |ui| {
                    for choice in SheetLayout::ALL {
                        ui.selectable_value(&mut sheet.layout, choice, choice.name());
                    }
                });
            ui.end_row();

            if sheet.layout != SheetLayout::Packed {
                ui.label(if sheet.layout == SheetLayout::Rows {
                    "Frames per row"
                } else {
                    "Frames per column"
                });
                ui.add(DragValue::new(&mut sheet.per_line).range(0..=1024))
                    .on_hover_text("0 puts every frame in one line");
                ui.end_row();
            }

            ui.label("Padding");
            ui.add(
                DragValue::new(&mut sheet.padding)
                    .range(0..=64)
                    .suffix(" px"),
            );
            ui.end_row();

            ui.label("");
            ui.checkbox(&mut sheet.trim, "Trim frames to what is drawn");
            ui.end_row();
            ui.label("");
            ui.checkbox(&mut json, "Write metadata (.json)");
            ui.end_row();
        } else {
            ui.label("Scale");
            ui.add(
                DragValue::new(&mut scale)
                    .range(1..=MAX_EXPORT_SCALE)
                    .suffix("×"),
            );
            ui.end_row();
        }
    });
    if kind == ExportKind::Gif {
        ui.label(RichText::new("A GIF has no half-transparent pixels: they become solid.").weak());
    }

    match buttons(ui, "Export…") {
        (true, _) => {
            let mut params = Params::new().with("tag", tag.map_or(-1, |at| at as i64));
            params = if kind == ExportKind::Sheet {
                params
                    .with("layout", sheet.layout.id())
                    .with("per_line", sheet.per_line)
                    .with("padding", sheet.padding)
                    .with("trim", sheet.trim)
                    .with("json", json)
            } else {
                params.with("scale", scale)
            };
            Outcome::Run(command.to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Export {
            kind,
            scale,
            tag,
            sheet,
            json,
        }),
    }
}

/// The selected slice: its name and rectangle, and the optional 9-slice centre and pivot.
fn slice(
    ui: &mut Ui,
    mut name: String,
    mut rect: Rect,
    mut center: Option<Rect>,
    mut pivot: Option<Point>,
) -> Outcome {
    ui.heading("Edit Slice");
    let rect_fields = |ui: &mut Ui, rect: &mut Rect| {
        ui.horizontal(|ui| {
            ui.add(DragValue::new(&mut rect.x).prefix("x "));
            ui.add(DragValue::new(&mut rect.y).prefix("y "));
            ui.add(
                DragValue::new(&mut rect.w)
                    .range(1..=MAX_SPRITE_SIZE)
                    .prefix("w "),
            );
            ui.add(
                DragValue::new(&mut rect.h)
                    .range(1..=MAX_SPRITE_SIZE)
                    .prefix("h "),
            );
        });
    };
    Grid::new("slice").num_columns(2).show(ui, |ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut name);
        ui.end_row();

        ui.label("Area");
        rect_fields(ui, &mut rect);
        ui.end_row();

        let mut nine = center.is_some();
        ui.checkbox(&mut nine, "9-slice centre")
            .on_hover_text("The part that stretches, measured from the slice's corner");
        match (nine, &mut center) {
            (true, Some(center)) => rect_fields(ui, center),
            // Switched on: start with the middle third.
            (true, None) => {
                center = Some(Rect::new(
                    (rect.w / 3) as i32,
                    (rect.h / 3) as i32,
                    (rect.w / 3).max(1),
                    (rect.h / 3).max(1),
                ));
            }
            (false, _) => center = None,
        }
        ui.end_row();

        let mut has_pivot = pivot.is_some();
        ui.checkbox(&mut has_pivot, "Pivot")
            .on_hover_text("A point to hold the slice by, measured from its corner");
        match (has_pivot, &mut pivot) {
            (true, Some(pivot)) => {
                ui.horizontal(|ui| {
                    ui.add(DragValue::new(&mut pivot.x).prefix("x "));
                    ui.add(DragValue::new(&mut pivot.y).prefix("y "));
                });
            }
            (true, None) => pivot = Some(Point::new((rect.w / 2) as i32, (rect.h / 2) as i32)),
            (false, _) => pivot = None,
        }
        ui.end_row();
    });

    match buttons(ui, "Change") {
        (true, _) => {
            let mut params = Params::new()
                .with("name", name.as_str())
                .with("x", i64::from(rect.x))
                .with("y", i64::from(rect.y))
                .with("width", rect.w)
                .with("height", rect.h)
                .with("nine", center.is_some())
                .with("pivot", pivot.is_some());
            if let Some(center) = center {
                params = params
                    .with("center_x", i64::from(center.x))
                    .with("center_y", i64::from(center.y))
                    .with("center_width", center.w)
                    .with("center_height", center.h);
            }
            if let Some(pivot) = pivot {
                params = params
                    .with("pivot_x", i64::from(pivot.x))
                    .with("pivot_y", i64::from(pivot.y));
            }
            Outcome::Run("slice.edit".to_owned(), params)
        }
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Slice {
            name,
            rect,
            center,
            pivot,
        }),
    }
}

/// How many colours the ramp from the primary to the secondary colour gets.
fn ramp(ui: &mut Ui, state: &AppState, mut steps: u32) -> Outcome {
    ui.heading("Generate Ramp");
    ui.label(format!(
        "From {} to {}. The colours are added to the palette.",
        state.editor.primary.to_hex(),
        state.editor.secondary.to_hex()
    ));
    Grid::new("ramp").num_columns(2).show(ui, |ui| {
        ui.label("Colours");
        ui.add(DragValue::new(&mut steps).range(2..=MAX_RAMP_STEPS as u32));
        ui.end_row();
    });

    match buttons(ui, "Generate") {
        (true, _) => Outcome::Run(
            "palette.generate_ramp".to_owned(),
            Params::new().with("steps", steps),
        ),
        (_, true) => Outcome::Dismissed,
        _ => Outcome::Open(Dialog::Ramp { steps }),
    }
}

/// Shows the native file dialog a command asked for, then runs the command with the chosen
/// path. This blocks until the user has picked a file or cancelled.
pub fn resolve_file_request(
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    parent: &eframe::Frame,
) {
    let Some(request) = state.file_request.take() else {
        return;
    };

    let mut dialog = rfd::FileDialog::new().set_parent(parent);
    // Start where the open sprite lives, with a name derived from it.
    if let Some(folder) = state.file_path.as_ref().and_then(|path| path.parent()) {
        dialog = dialog.set_directory(folder);
    }
    let stem = state
        .file_path
        .as_ref()
        .and_then(|path| path.file_stem())
        .map_or_else(
            || "untitled".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        );
    let with_extension = |path: PathBuf, extension: &str| match path.extension() {
        Some(_) => path,
        None => path.with_extension(extension),
    };
    let text = |path: PathBuf| path.to_string_lossy().into_owned();

    let (command, params) = match request {
        FileRequest::ImportAseprite => {
            let picked = dialog
                .add_filter("Aseprite file", &["aseprite", "ase"])
                .pick_file();
            let Some(path) = picked else { return };
            (
                "file.import_aseprite",
                Params::new().with("path", text(path)),
            )
        }
        FileRequest::Open => {
            let picked = dialog
                .add_filter("Sprites", &["acms", "aseprite", "ase"])
                .add_filter("ACMS sprite", &["acms"])
                .pick_file();
            let Some(path) = picked else { return };
            ("file.open", Params::new().with("path", text(path)))
        }
        FileRequest::SaveAs => {
            let picked = dialog
                .add_filter("ACMS sprite", &["acms"])
                .set_file_name(format!("{stem}.acms"))
                .save_file();
            let Some(path) = picked else { return };
            let path = with_extension(path, "acms");
            ("file.save_as", Params::new().with("path", text(path)))
        }
        FileRequest::ExportPng { scale } => {
            let picked = dialog
                .add_filter("PNG image", &["png"])
                .set_file_name(format!("{stem}.png"))
                .save_file();
            let Some(path) = picked else { return };
            let path = with_extension(path, "png");
            let params = Params::new().with("path", text(path)).with("scale", scale);
            ("file.export_png", params)
        }
        FileRequest::ImportPalette => {
            let picked = dialog
                .add_filter("Palettes and pictures", &["gpl", "hex", "pal", "png"])
                .pick_file();
            let Some(path) = picked else { return };
            ("palette.import", Params::new().with("path", text(path)))
        }
        FileRequest::ExportPalette => {
            let picked = dialog
                .add_filter("GIMP palette", &["gpl"])
                .add_filter("Hex list", &["hex"])
                .add_filter("JASC palette", &["pal"])
                .set_file_name(format!("{stem}.gpl"))
                .save_file();
            let Some(path) = picked else { return };
            let path = with_extension(path, "gpl");
            ("palette.export", Params::new().with("path", text(path)))
        }
        FileRequest::Export {
            command,
            params,
            file_type: (label, extension),
        } => {
            let picked = dialog
                .add_filter(label, &[extension])
                .set_file_name(format!("{stem}.{extension}"))
                .save_file();
            let Some(path) = picked else { return };
            let path = with_extension(path, extension);
            (command, params.with("path", text(path)))
        }
        FileRequest::ReferenceImage => {
            let picked = dialog.add_filter("PNG image", &["png"]).pick_file();
            let Some(path) = picked else { return };
            (
                "layer.add_reference",
                Params::new().with("path", text(path)),
            )
        }
    };
    commands::run(registry, state, command, &params);
}
