//! The eframe application: wires the registry, keymap, palette and dock together.

use acms_core::command::CommandRegistry;
use egui_dock::DockArea;

use crate::commands::{self, AppState};
use crate::keymap::Keymap;
use crate::palette;
use crate::panels::PanelViewer;
use crate::theme::{self, ThemeKind};

pub struct AcmsApp {
    registry: CommandRegistry<AppState>,
    keymap: Keymap,
    state: AppState,
    /// The theme egui is currently showing; compared with `state.theme` every frame.
    shown_theme: ThemeKind,
}

impl AcmsApp {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<Self> {
        let mut registry = CommandRegistry::new();
        commands::register_builtin(&mut registry)?;

        let state = AppState::default();
        theme::install(ctx, state.theme);

        Ok(Self {
            registry,
            keymap: Keymap::with_defaults(),
            shown_theme: state.theme,
            state,
        })
    }

    fn run_command(&mut self, id: &str) {
        if let Err(err) = self.registry.run(id, &mut self.state) {
            self.state.log.push(err.to_string());
        }
    }

    /// One frame of the whole UI. Separate from [`eframe::App::ui`] so tests can drive it
    /// without a window.
    fn frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        if let Some(id) = self.keymap.triggered(&ctx).map(str::to_owned) {
            self.run_command(&id);
        }

        let palette_shortcut = self
            .keymap
            .shortcuts_for("view.command_palette")
            .next()
            .map(|shortcut| ctx.format_shortcut(shortcut))
            .unwrap_or_default();

        let surface = self.shown_theme.tokens().surface;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(surface))
            .show(ui, |ui| {
                let mut viewer = PanelViewer {
                    log: &self.state.log,
                    palette_shortcut: &palette_shortcut,
                };
                DockArea::new(&mut self.state.dock)
                    .style(theme::dock_style(ui.style(), self.shown_theme))
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut viewer);
            });

        // The palette needs its own state mutably and the rest of the app state read-only.
        let mut palette_state = std::mem::take(&mut self.state.palette);
        let picked = palette::show(
            &ctx,
            &mut palette_state,
            &self.registry,
            &self.keymap,
            &self.state,
        );
        self.state.palette = palette_state;
        if let Some(id) = picked {
            self.run_command(&id);
        }

        if self.state.theme != self.shown_theme {
            self.shown_theme = self.state.theme;
            theme::set_active(&ctx, self.shown_theme);
        }
        if std::mem::take(&mut self.state.quit_requested) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for AcmsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

#[cfg(test)]
mod tests {
    use egui::{Event, Key, Modifiers, RawInput};

    use super::*;

    /// Ctrl as a desktop backend reports it on Windows and Linux.
    const CTRL: Modifiers = Modifiers::CTRL.plus(Modifiers::COMMAND);

    struct Harness {
        ctx: egui::Context,
        app: AcmsApp,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            let app = AcmsApp::new(&ctx).unwrap();
            let mut harness = Self { ctx, app };
            harness.frame(vec![]);
            harness
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            let input = RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| self.app.frame(ui));
            // There is no renderer to upload the font atlas to.
            output.textures_delta.clear();
            output
        }

        fn press(&mut self, modifiers: Modifiers, key: Key) {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
            self.frame(vec![]);
        }

        fn type_text(&mut self, text: &str) {
            self.frame(vec![Event::Text(text.to_owned())]);
        }
    }

    #[test]
    fn a_shortcut_runs_its_command() {
        let mut h = Harness::new();
        h.press(CTRL, Key::H);
        assert_eq!(h.app.state.log, ["Hello from the command system"]);
    }

    #[test]
    fn ctrl_shift_p_and_ctrl_k_open_the_palette() {
        for (modifiers, key) in [(CTRL.plus(Modifiers::SHIFT), Key::P), (CTRL, Key::K)] {
            let mut h = Harness::new();
            assert!(!h.app.state.palette.is_open());
            h.press(modifiers, key);
            assert!(h.app.state.palette.is_open());
        }
    }

    #[test]
    fn palette_search_then_enter_runs_the_command_and_closes() {
        let mut h = Harness::new();
        h.press(CTRL.plus(Modifiers::SHIFT), Key::P);
        h.type_text("hello");
        h.press(Modifiers::NONE, Key::Enter);
        assert_eq!(h.app.state.log, ["Hello from the command system"]);
        assert!(!h.app.state.palette.is_open());
    }

    #[test]
    fn palette_arrow_keys_move_the_selection() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        // The second entry in registration order is "Toggle Light / Dark Theme".
        h.press(Modifiers::NONE, Key::ArrowDown);
        h.press(Modifiers::NONE, Key::Enter);
        assert_eq!(h.app.state.theme, ThemeKind::Light);
        assert_eq!(h.ctx.theme(), egui::Theme::Light);
    }

    #[test]
    fn escape_closes_the_palette_without_running_anything() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        h.press(Modifiers::NONE, Key::Escape);
        assert!(!h.app.state.palette.is_open());
        assert!(h.app.state.log.is_empty());
    }

    #[test]
    fn a_disabled_command_cannot_be_run_from_the_palette() {
        let mut h = Harness::new();
        h.press(CTRL, Key::K);
        h.type_text("clear log");
        h.press(Modifiers::NONE, Key::Enter);
        // Nothing to clear, so the command is disabled: the palette stays open.
        assert!(h.app.state.palette.is_open());
        assert!(h.app.state.log.is_empty());
    }

    #[test]
    fn the_quit_shortcut_requests_the_window_to_close() {
        let mut h = Harness::new();
        let output = h.frame(vec![Event::Key {
            key: Key::Q,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: CTRL,
        }]);
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.contains(&egui::ViewportCommand::Close));
    }
}
