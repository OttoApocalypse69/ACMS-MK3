//! ACMS mk3 — the desktop app.

// No console window next to the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod fuzzy;
mod keymap;
mod palette;
mod panels;
mod theme;

fn main() -> anyhow::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ACMS mk3")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "acms-mk3",
        options,
        Box::new(|cc| Ok(Box::new(app::AcmsApp::new(&cc.egui_ctx)?))),
    )
    .map_err(|err| anyhow::anyhow!("could not run the app: {err}"))
}
