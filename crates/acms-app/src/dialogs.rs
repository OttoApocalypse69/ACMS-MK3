//! Modal dialogs and native file dialogs.
//!
//! A dialog collects what a command needs and then runs that command; it changes nothing else.

use std::path::PathBuf;

use acms_core::command::{CommandRegistry, Params};
use acms_core::sprite::MAX_SPRITE_SIZE;
use acms_io::png::MAX_EXPORT_SCALE;
use egui::{DragValue, Grid, Id, Modal, RichText, Ui};

use crate::commands;
use crate::state::{AppState, Dialog, FileRequest};

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
        FileRequest::Open { discard } => {
            let picked = dialog.add_filter("ACMS sprite", &["acms"]).pick_file();
            let Some(path) = picked else { return };
            let params = Params::new()
                .with("path", text(path))
                .with("discard", discard);
            ("file.open", params)
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
    };
    commands::run(registry, state, command, &params);
}
