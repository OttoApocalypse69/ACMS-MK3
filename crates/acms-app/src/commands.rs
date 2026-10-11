//! The state commands act on, and the built-in commands.
//!
//! UI code never changes [`AppState`] directly: it asks the registry to run a command.

use acms_core::command::{Command, CommandError, CommandRegistry};
use egui_dock::DockState;

use crate::palette::PaletteState;
use crate::panels::{self, Panel};
use crate::theme::ThemeKind;

pub struct AppState {
    pub dock: DockState<Panel>,
    pub theme: ThemeKind,
    pub palette: PaletteState,
    /// Lines shown in the Log panel.
    pub log: Vec<String>,
    /// Set by `app.quit`; the app closes the window at the end of the frame.
    pub quit_requested: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            dock: panels::default_layout(),
            theme: ThemeKind::default(),
            palette: PaletteState::default(),
            log: Vec::new(),
            quit_requested: false,
        }
    }
}

/// Registers the Phase 0 commands. The `debug.*` ones are dummies that exist to exercise the
/// registry, keymap and palette.
pub fn register_builtin(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    registry.register(Command::new(
        "view.command_palette",
        "Command Palette",
        |state: &mut AppState| state.palette.open(),
    ))?;
    registry.register(Command::new(
        "view.toggle_theme",
        "Toggle Light / Dark Theme",
        |state: &mut AppState| {
            state.theme = state.theme.toggled();
            state.log.push(format!("Theme: {}", state.theme.name()));
        },
    ))?;
    registry.register(Command::new(
        "view.reset_layout",
        "Reset Panel Layout",
        |state: &mut AppState| {
            state.dock = panels::default_layout();
            state.log.push("Panel layout reset".to_owned());
        },
    ))?;
    registry.register(Command::new(
        "debug.say_hello",
        "Debug: Say Hello",
        |state: &mut AppState| state.log.push("Hello from the command system".to_owned()),
    ))?;
    registry.register(
        Command::new(
            "debug.clear_log",
            "Debug: Clear Log",
            |state: &mut AppState| {
                state.log.clear();
            },
        )
        .enabled_when(|state| !state.log.is_empty()),
    )?;
    registry.register(Command::new("app.quit", "Quit", |state: &mut AppState| {
        state.quit_requested = true;
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap;

    fn registry() -> CommandRegistry<AppState> {
        let mut registry = CommandRegistry::new();
        register_builtin(&mut registry).unwrap();
        registry
    }

    #[test]
    fn every_default_shortcut_points_at_a_registered_command() {
        let registry = registry();
        for command in keymap::default_commands() {
            assert!(registry.get(command).is_some(), "no command `{command}`");
        }
    }

    #[test]
    fn toggle_theme_flips_the_theme_and_logs() {
        let registry = registry();
        let mut state = AppState::default();
        registry.run("view.toggle_theme", &mut state).unwrap();
        assert_eq!(state.theme, ThemeKind::Light);
        assert_eq!(state.log, ["Theme: Light"]);
    }

    #[test]
    fn clear_log_is_only_enabled_when_there_is_something_to_clear() {
        let registry = registry();
        let mut state = AppState::default();
        assert_eq!(
            registry.run("debug.clear_log", &mut state),
            Err(CommandError::Disabled("debug.clear_log".to_owned()))
        );
        registry.run("debug.say_hello", &mut state).unwrap();
        registry.run("debug.clear_log", &mut state).unwrap();
        assert!(state.log.is_empty());
    }

    #[test]
    fn reset_layout_brings_closed_panels_back() {
        let registry = registry();
        let mut state = AppState::default();
        let before = state.dock.iter_all_tabs().count();
        state.dock.retain_tabs(|tab| *tab != Panel::Log);
        assert_eq!(state.dock.iter_all_tabs().count(), before - 1);

        registry.run("view.reset_layout", &mut state).unwrap();
        assert_eq!(state.dock.iter_all_tabs().count(), before);
    }

    #[test]
    fn palette_and_quit_commands_set_their_flags() {
        let registry = registry();
        let mut state = AppState::default();
        registry.run("view.command_palette", &mut state).unwrap();
        assert!(state.palette.is_open());
        registry.run("app.quit", &mut state).unwrap();
        assert!(state.quit_requested);
    }
}
