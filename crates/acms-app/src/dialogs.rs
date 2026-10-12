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
use crate::state::{Anchor, AppState, Dialog, ExportKind, FileRequest, SizePurpose};

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
            Dialog::Tag {
                index,
                name,
                from,
                to,
                direction,
                color,
            } => tag(ui, state, index, name, from, to, direction, color),
            Dialog::FrameDuration { ms } => frame_duration(ui, ms),
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
