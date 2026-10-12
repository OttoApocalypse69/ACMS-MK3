//! ACMS mk3 — the desktop app.

// No console window next to the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim_commands;
mod app;
mod canvas;
mod clipboard;
mod commands;
mod config;
mod dialogs;
mod edit_commands;
mod fuzzy;
mod keymap;
mod layer_commands;
mod layers_panel;
mod menu;
mod palette;
mod panels;
mod pixel_commands;
mod preview;
mod state;
mod status;
mod theme;
mod tile_commands;
mod timeline;

fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ACMS mk3")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    let keymap_file = config::keymap_path();

    eframe::run_native(
        "acms-mk3",
        options,
        Box::new(|cc| {
            let mut app = app::AcmsApp::new(&cc.egui_ctx, keymap_file.as_deref())?;
            app.use_system_clipboard();
            Ok(Box::new(app))
        }),
    )
    .map_err(|err| anyhow::anyhow!("could not run the app: {err}"))
}
