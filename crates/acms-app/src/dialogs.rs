//! Modal dialogs and native file dialogs.
//!
//! A dialog collects what a command needs and then runs that command; it changes nothing else.

use std::path::PathBuf;

use acms_core::command::{CommandRegistry, Params};
use acms_core::palette::MAX_RAMP_STEPS;
use acms_core::sprite::MAX_SPRITE_SIZE;
use acms_io::png::MAX_EXPORT_SCALE;
use egui::{DragValue, Grid, Id, Modal, RichText, Ui};

use crate::commands;
use crate::state::{Anchor, AppState, Dialog, FileRequest, SizePurpose};

/// What the user did with a dialog this frame.
enum Outcome {
    /// Nothing yet; keep showing it.
    Open(Dialog),
    Dismissed,
    Run(String, Params),
}

/// Shows the open dialog, if any.
pub fn show(ctx: &egui::Context, state: &mut AppState, registry: &CommandRegistry<AppState>) {
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
        }
    });

    // Clicking outside the dialog or pressing Escape is the same as Cancel.
    let dismissed = modal.should_close();
    match modal.inner {
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
    if purpose != SizePurpose::CanvasSize {
        ui.label(RichText::new("Pixels are repeated or dropped, never blended.").weak());
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
        FileRequest::Open => {
            let picked = dialog.add_filter("ACMS sprite", &["acms"]).pick_file();
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
