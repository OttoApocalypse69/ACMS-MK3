//! What the app remembers between runs: preferences, recent files and the panel layout; and
//! the commands that change them and the keymap.
//!
//! Each lives in its own small JSON file in the user's configuration folder. A missing or
//! broken file is never fatal: the defaults are used and the Log panel says why.

use std::path::{Path, PathBuf};

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use egui_dock::DockState;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::panels::{self, Panel};
use crate::state::{AppState, Dialog};
use crate::theme::ThemeKind;

type Registry = CommandRegistry<AppState>;

/// Most files the Open Recent list holds.
const MAX_RECENT: usize = 10;

/// Where the app keeps its files. Any of them may be missing: then that thing is neither
/// loaded nor saved, which is how the tests run.
#[derive(Clone, Debug, Default)]
pub struct Paths {
    pub keymap: Option<PathBuf>,
    /// The folder of the user's scripts.
    pub scripts: Option<PathBuf>,
    pub preferences: Option<PathBuf>,
    /// Recent files and the panel layout.
    pub session: Option<PathBuf>,
    /// The folder unsaved sprites are autosaved into.
    pub recovery: Option<PathBuf>,
}

impl Paths {
    /// The files of the user who runs the app.
    pub fn user() -> Self {
        let folder = config::keymap_path().and_then(|path| path.parent().map(Path::to_owned));
        Self {
            keymap: config::keymap_path(),
            scripts: config::scripts_dir(),
            preferences: folder.as_ref().map(|dir| dir.join("preferences.json")),
            session: folder.as_ref().map(|dir| dir.join("session.json")),
            recovery: config::recovery_dir(),
        }
    }

    /// Everything inside one folder.
    #[cfg(test)]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            keymap: Some(dir.join("keymap.json")),
            scripts: Some(dir.join("scripts")),
            preferences: Some(dir.join("preferences.json")),
            session: Some(dir.join("session.json")),
            recovery: Some(dir.join("recovery")),
        }
    }
}

/// The user's preferences.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: ThemeKind,
    /// How large the interface is drawn; 1 is the size the system suggests.
    pub ui_scale: f32,
    /// How much memory the undo history of one sprite may use, in megabytes.
    pub history_mb: u32,
    /// How often sprites with unsaved changes are saved to the recovery folder, in minutes.
    /// 0 turns it off.
    pub autosave_minutes: u32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemeKind::Dark,
            ui_scale: 1.0,
            history_mb: 256,
            autosave_minutes: 5,
        }
    }
}

impl Preferences {
    pub const UI_SCALES: std::ops::RangeInclusive<f32> = 0.75..=2.0;
    pub const HISTORY_MB: std::ops::RangeInclusive<u32> = 16..=4096;
    pub const AUTOSAVE_MINUTES: std::ops::RangeInclusive<u32> = 0..=120;

    /// The same preferences with every value brought into its range.
    pub fn sanitized(self) -> Self {
        let scale = if self.ui_scale.is_finite() {
            self.ui_scale
        } else {
            1.0
        };
        Self {
            theme: self.theme,
            ui_scale: scale.clamp(*Self::UI_SCALES.start(), *Self::UI_SCALES.end()),
            history_mb: self
                .history_mb
                .clamp(*Self::HISTORY_MB.start(), *Self::HISTORY_MB.end()),
            autosave_minutes: self.autosave_minutes.min(*Self::AUTOSAVE_MINUTES.end()),
        }
    }
}

/// What `session.json` holds.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Session {
    recent: Vec<PathBuf>,
    /// A [`DockState`]. Kept as loose JSON so that a layout that cannot be read, say from
    /// another version of the docking library, does not take the rest of the file with it.
    layout: Option<serde_json::Value>,
}

/// Makes a layout written as JSON readable again. The layout holds where each panel was on
/// screen, and for panels that were never shown those numbers are infinite, which JSON
/// cannot say: they come out as `null`. They are worked out again when the layout is shown,
/// so any number will do.
fn mend(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, field) in fields.iter_mut() {
                if field.is_null() && matches!(name.as_str(), "x" | "y") {
                    *field = serde_json::Value::from(0.0);
                } else {
                    mend(field);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(mend),
        _ => {}
    }
}

/// A change to the keymap a command asked for. The keymap is not part of the state commands
/// act on, so the app carries these out at the start of the next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeymapChange {
    Bind {
        command: String,
        keys: String,
    },
    Unbind {
        keys: String,
    },
    /// Take every chord away from a command.
    Clear {
        command: String,
    },
    /// Back to the default keys: for one command, or for all.
    Reset(Option<String>),
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| format!("{} is not valid ({err})", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("could not read {} ({err})", path.display())),
    }
}

/// Writes a small settings file, by way of a temporary file so that a crash halfway leaves
/// the old one.
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let mut temp = path.as_os_str().to_owned();
        temp.push(".tmp");
        let temp = PathBuf::from(temp);
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)
    };
    write().map_err(|err| format!("could not write {} ({err})", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|err| err.to_string())?;
    write_text(path, &text)
}

/// Whether a layout read from a file can be used: the canvas must be in it, since it is the
/// one panel that cannot be brought back by hand.
fn usable(layout: &DockState<Panel>) -> bool {
    layout.find_tab(&Panel::Canvas).is_some()
}

/// Reads the preferences, the recent files and the layout. Called once, at start.
pub fn load(state: &mut AppState) {
    if let Some(path) = state.paths.preferences.clone() {
        match read_json::<Preferences>(&path) {
            Ok(Some(prefs)) => state.prefs = prefs.sanitized(),
            Ok(None) => {}
            Err(why) => state
                .log
                .push(format!("Preferences: {why}; using the defaults")),
        }
    }
    state.theme = state.prefs.theme;

    if let Some(path) = state.paths.session.clone() {
        match read_json::<Session>(&path) {
            Ok(Some(session)) => {
                state.recent = session.recent;
                state.recent.truncate(MAX_RECENT);
                if let Some(mut layout) = session.layout {
                    mend(&mut layout);
                    match serde_json::from_value::<DockState<Panel>>(layout) {
                        Ok(layout) if usable(&layout) => {
                            state.dock = layout;
                            state.layout_generation += 1;
                        }
                        Ok(_) => {}
                        Err(err) => state
                            .log
                            .push(format!("Session: the saved layout was not used ({err})")),
                    }
                }
            }
            Ok(None) => {}
            Err(why) => state.log.push(format!("Session: {why}; starting fresh")),
        }
    }
}

pub fn save_preferences(state: &mut AppState) {
    state.prefs.theme = state.theme;
    let Some(path) = state.paths.preferences.clone() else {
        return;
    };
    if let Err(why) = write_json(&path, &state.prefs) {
        state.log.push(format!("Preferences: {why}"));
    }
}

/// Saves the recent files and the layout as they are now.
pub fn save_session(state: &mut AppState) {
    let Some(path) = state.paths.session.clone() else {
        return;
    };
    let session = Session {
        recent: state.recent.clone(),
        layout: serde_json::to_value(&state.dock).ok(),
    };
    if let Err(why) = write_json(&path, &session) {
        state.log.push(format!("Session: {why}"));
    }
}

/// Puts a file at the top of the Open Recent list.
pub fn note_recent(state: &mut AppState, path: &Path) {
    state.recent.retain(|known| known != path);
    state.recent.insert(0, path.to_owned());
    state.recent.truncate(MAX_RECENT);
    save_session(state);
}

/// Gives every open sprite the undo memory the preferences allow.
pub fn apply_history_limit(state: &mut AppState) {
    let bytes = state.prefs.history_mb as usize * 1024 * 1024;
    let ids: Vec<_> = state.editor.documents().map(|(id, _)| id).collect();
    for id in ids {
        if let Some(document) = state.editor.document_mut_by_id(id) {
            document.set_history_limit(bytes);
        }
    }
}

fn set_layout(state: &mut AppState, layout: DockState<Panel>) {
    state.dock = layout;
    state.layout_generation += 1;
    save_session(state);
}

fn prefs_set(state: &mut AppState, params: &Params) -> Result<(), CommandError> {
    let mut prefs = state.prefs;
    prefs.theme = state.theme;
    match params.text("theme") {
        Some("dark") => prefs.theme = ThemeKind::Dark,
        Some("light") => prefs.theme = ThemeKind::Light,
        Some(other) => {
            return Err(CommandError::failed(format!(
                "The theme is \"dark\" or \"light\", not \"{other}\"."
            )));
        }
        None => {}
    }
    if let Some(scale) = params.float("ui_scale") {
        prefs.ui_scale = scale as f32;
    }
    let whole = |name: &str, current: u32| match params.int(name) {
        Some(value) => u32::try_from(value.max(0)).unwrap_or(u32::MAX),
        None => current,
    };
    prefs.history_mb = whole("history_mb", prefs.history_mb);
    prefs.autosave_minutes = whole("autosave_minutes", prefs.autosave_minutes);

    state.prefs = prefs.sanitized();
    state.theme = state.prefs.theme;
    apply_history_limit(state);
    save_preferences(state);
    Ok(())
}

pub fn register_commands(registry: &mut Registry) -> Result<(), CommandError> {
    use ParamKind::{Float, Int, Text};

    registry.register(Command::new(
        "app.preferences",
        "Preferences…",
        |state: &mut AppState, _| {
            let mut draft = state.prefs;
            draft.theme = state.theme;
            state.dialog = Some(Dialog::Preferences(draft));
            Ok(())
        },
    ))?;
    registry.register(
        Command::new("prefs.set", "Set Preferences", prefs_set)
            .optional_param("theme", Text)
            .optional_param("ui_scale", Float)
            .optional_param("history_mb", Int)
            .optional_param("autosave_minutes", Int),
    )?;

    // Layouts. `view.reset_layout`, the default one, is with the other view commands.
    registry.register(Command::new(
        "view.layout_animation",
        "Layout: Animation",
        |state: &mut AppState, _| {
            set_layout(state, panels::animation_layout());
            Ok(())
        },
    ))?;
    registry.register(Command::new(
        "view.layout_drawing",
        "Layout: Drawing",
        |state: &mut AppState, _| {
            set_layout(state, panels::drawing_layout());
            Ok(())
        },
    ))?;

    registry.register(
        Command::new(
            "file.clear_recent",
            "Clear Recent Files",
            |state: &mut AppState, _| {
                state.recent.clear();
                save_session(state);
                Ok(())
            },
        )
        .enabled_when(|state| !state.recent.is_empty()),
    )?;

    // The keymap.
    registry.register(Command::new(
        "keymap.edit",
        "Keyboard Shortcuts…",
        |state: &mut AppState, _| {
            state.dialog = Some(Dialog::Keymap {
                filter: String::new(),
                capture: None,
                note: String::new(),
            });
            Ok(())
        },
    ))?;
    registry.register(
        Command::new(
            "keymap.bind",
            "Bind Keys to a Command",
            |state: &mut AppState, params| {
                let command = params.text("command").unwrap_or_default().to_owned();
                let keys = params.text("keys").unwrap_or_default().to_owned();
                if crate::keymap::parse_chord(&keys).is_none() {
                    return Err(CommandError::failed(format!(
                        "\"{keys}\" is not a key chord."
                    )));
                }
                state
                    .keymap_changes
                    .push(KeymapChange::Bind { command, keys });
                Ok(())
            },
        )
        .param("command", Text)
        .param("keys", Text),
    )?;
    registry.register(
        Command::new(
            "keymap.unbind",
            "Unbind Keys",
            |state: &mut AppState, params| {
                let keys = params.text("keys").unwrap_or_default().to_owned();
                if crate::keymap::parse_chord(&keys).is_none() {
                    return Err(CommandError::failed(format!(
                        "\"{keys}\" is not a key chord."
                    )));
                }
                state.keymap_changes.push(KeymapChange::Unbind { keys });
                Ok(())
            },
        )
        .param("keys", Text),
    )?;
    registry.register(
        Command::new(
            "keymap.clear",
            "Remove the Keys of a Command",
            |state: &mut AppState, params| {
                let command = params.text("command").unwrap_or_default().to_owned();
                state.keymap_changes.push(KeymapChange::Clear { command });
                Ok(())
            },
        )
        .param("command", Text),
    )?;
    registry.register(
        Command::new(
            "keymap.reset",
            "Reset Keyboard Shortcuts",
            |state: &mut AppState, params| {
                let command = params.text("command").map(str::to_owned);
                state.keymap_changes.push(KeymapChange::Reset(command));
                Ok(())
            },
        )
        .optional_param("command", Text),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use acms_core::command::Params;

    use super::*;
    use crate::commands::testing::Fixture;
    use crate::config::TempDir;
    use crate::state::FileRequest;

    /// A fixture whose files live in `dir`, loaded as at start.
    fn fixture_in(dir: &TempDir) -> Fixture {
        let mut f = Fixture::new();
        f.state.paths = Paths::in_dir(dir.path());
        load(&mut f.state);
        f
    }

    fn sample(name: &str) -> String {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples");
        root.join(name).to_string_lossy().into_owned()
    }

    #[test]
    fn preferences_are_set_checked_saved_and_read_back() {
        let dir = TempDir::new("prefs");
        let mut f = fixture_in(&dir);
        assert_eq!(f.state.prefs, Preferences::default());
        assert!(f.state.log.is_empty());

        f.run("app.preferences");
        assert_eq!(
            f.state.dialog,
            Some(Dialog::Preferences(Preferences::default()))
        );
        f.state.dialog = None;

        let params = Params::new()
            .with("theme", "light")
            .with("ui_scale", 1.5)
            .with("history_mb", 64_i64)
            .with("autosave_minutes", 0_i64);
        f.run_with("prefs.set", params);
        let expected = Preferences {
            theme: ThemeKind::Light,
            ui_scale: 1.5,
            history_mb: 64,
            autosave_minutes: 0,
        };
        assert_eq!(f.state.prefs, expected);
        assert_eq!(f.state.theme, ThemeKind::Light);
        // The next run starts with them.
        assert_eq!(fixture_in(&dir).state.prefs, expected);
        assert_eq!(fixture_in(&dir).state.theme, ThemeKind::Light);

        // Values out of range are brought into it; one setting can be changed alone.
        let params = Params::new()
            .with("ui_scale", 99.0)
            .with("history_mb", -5_i64)
            .with("autosave_minutes", 100_000_i64);
        f.run_with("prefs.set", params);
        assert_eq!(
            (
                f.state.prefs.ui_scale,
                f.state.prefs.history_mb,
                f.state.prefs.autosave_minutes
            ),
            (2.0, 16, 120)
        );
        assert_eq!(f.state.prefs.theme, ThemeKind::Light);
        assert!(
            f.try_run("prefs.set", Params::new().with("theme", "pink"))
                .is_err()
        );

        // The theme shortcut is remembered too.
        f.run("view.toggle_theme");
        assert_eq!(fixture_in(&dir).state.theme, ThemeKind::Dark);
    }

    #[test]
    fn a_broken_or_odd_preferences_file_means_defaults() {
        let dir = TempDir::new("prefs-broken");
        let path = dir.path().join("preferences.json");
        std::fs::write(&path, "{ not json").unwrap();
        let f = fixture_in(&dir);
        assert_eq!(f.state.prefs, Preferences::default());
        assert!(
            f.state.log[0].contains("using the defaults"),
            "{:?}",
            f.state.log
        );

        // A file from another version: what is known is used, what is odd is put right.
        std::fs::write(
            &path,
            r#"{ "theme": "light", "ui_scale": 50, "later": true }"#,
        )
        .unwrap();
        let f = fixture_in(&dir);
        assert_eq!(f.state.prefs.theme, ThemeKind::Light);
        assert_eq!(f.state.prefs.ui_scale, 2.0);
        assert_eq!(f.state.prefs.history_mb, 256);
        assert!(f.state.log.is_empty());
    }

    #[test]
    fn the_undo_memory_reaches_open_and_new_sprites() {
        let mut f = Fixture::with_sprite();
        let limit = |f: &Fixture| f.document().history_limit() / (1024 * 1024);
        assert_eq!(limit(&f), 256);
        f.run_with("prefs.set", Params::new().with("history_mb", 32_i64));
        assert_eq!(limit(&f), 32);
        let size = Params::new().with("width", 4_u32).with("height", 4_u32);
        f.run_with("file.new", size);
        assert_eq!(limit(&f), 32);
        assert_eq!(f.state.editor.documents().count(), 2);
    }

    #[test]
    fn recent_files_are_listed_newest_first_and_remembered() {
        let dir = TempDir::new("recent");
        let mut f = fixture_in(&dir);
        assert!(!f.is_enabled("file.clear_recent"));
        let size = Params::new().with("width", 4_u32).with("height", 4_u32);
        let path = |index: usize| dir.path().join(format!("sprite{index}.acms"));
        let text = |index: usize| path(index).to_string_lossy().into_owned();
        for index in 0..12 {
            f.run_with("file.new", size.clone());
            f.run_with("file.save_as", Params::new().with("path", text(index)));
        }
        // The ten newest, newest first.
        assert_eq!(f.state.recent.len(), 10);
        assert_eq!(f.state.recent[0], path(11));
        assert_eq!(f.state.recent[9], path(2));

        // Opening one again moves it to the top; it is not listed twice.
        let mut f = fixture_in(&dir);
        assert_eq!(f.state.recent.len(), 10);
        f.run_with("file.open", Params::new().with("path", text(5)));
        assert_eq!(f.state.recent[0], path(5));
        assert_eq!(f.state.recent.iter().filter(|p| **p == path(5)).count(), 1);
        assert_eq!(f.state.recent.len(), 10);
        // A file that could not be opened is not listed.
        f.run_with("file.open", Params::new().with("path", text(99)));
        f.state.dialog = None;
        assert_eq!(f.state.recent[0], path(5));

        assert_eq!(fixture_in(&dir).state.recent, f.state.recent);
        f.run("file.clear_recent");
        assert!(f.state.recent.is_empty());
        assert!(fixture_in(&dir).state.recent.is_empty());
    }

    #[test]
    fn layouts_are_chosen_saved_and_restored() {
        let dir = TempDir::new("layout");
        let mut f = fixture_in(&dir);
        let panels = |state: &AppState| -> Vec<Panel> {
            let mut all: Vec<Panel> = state.dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
            all.sort_by_key(|panel| format!("{panel:?}"));
            all
        };
        let default = panels(&f.state);

        for id in ["view.layout_animation", "view.layout_drawing"] {
            let generation = f.state.layout_generation;
            f.run(id);
            assert_ne!(f.state.layout_generation, generation, "{id}");
            assert!(f.state.dock.find_tab(&Panel::Canvas).is_some(), "{id}");
            assert!(f.state.dock.find_tab(&Panel::Tools).is_some(), "{id}");
            assert!(f.state.dock.find_tab(&Panel::Layers).is_some(), "{id}");
        }
        // The drawing layout leaves the animation panels out.
        assert!(f.state.dock.find_tab(&Panel::Timeline).is_none());
        let drawing = panels(&f.state);
        assert_ne!(drawing, default);

        // The next run starts with the layout that was chosen.
        assert_eq!(panels(&fixture_in(&dir).state), drawing);

        f.run("view.reset_layout");
        assert_eq!(panels(&f.state), default);
        assert_eq!(panels(&fixture_in(&dir).state), default);

        // A panel the user closed stays closed next time.
        let log = f.state.dock.find_tab(&Panel::Log).unwrap();
        f.state.dock.remove_tab(log);
        save_session(&mut f.state);
        let reopened = fixture_in(&dir);
        assert!(reopened.state.dock.find_tab(&Panel::Log).is_none());
        assert_eq!(panels(&reopened.state).len(), default.len() - 1);
    }

    #[test]
    fn a_layout_without_a_canvas_is_not_used() {
        let dir = TempDir::new("layout-bad");
        let mut f = fixture_in(&dir);
        f.state.dock = DockState::new(vec![Panel::Log]);
        f.state.recent = vec![PathBuf::from("kept.acms")];
        save_session(&mut f.state);
        let reopened = fixture_in(&dir);
        assert!(reopened.state.dock.find_tab(&Panel::Canvas).is_some());
        // The rest of the file is still used.
        assert_eq!(reopened.state.recent, [PathBuf::from("kept.acms")]);

        std::fs::write(dir.path().join("session.json"), "[1, 2").unwrap();
        let broken = fixture_in(&dir);
        assert!(
            broken.state.log[0].contains("starting fresh"),
            "{:?}",
            broken.state.log
        );
        assert!(broken.state.dock.find_tab(&Panel::Canvas).is_some());
    }

    #[test]
    fn without_a_place_for_them_nothing_is_written() {
        let mut f = Fixture::with_sprite();
        f.run_with("prefs.set", Params::new().with("theme", "light"));
        f.run("view.layout_drawing");
        f.run("view.toggle_theme");
        assert!(f.state.log.is_empty(), "{:?}", f.state.log);
    }

    #[test]
    fn keymap_commands_ask_the_app_for_the_change() {
        let mut f = Fixture::new();
        f.run("keymap.edit");
        assert!(matches!(
            f.state.dialog,
            Some(Dialog::Keymap { capture: None, .. })
        ));
        f.state.dialog = None;

        let bind = Params::new()
            .with("command", "layer.new")
            .with("keys", "Ctrl+J");
        f.run_with("keymap.bind", bind);
        f.run_with("keymap.unbind", Params::new().with("keys", "Ctrl+G"));
        f.run_with("keymap.clear", Params::new().with("command", "edit.undo"));
        f.run_with("keymap.reset", Params::new().with("command", "edit.undo"));
        f.run("keymap.reset");
        assert_eq!(
            f.state.keymap_changes,
            [
                KeymapChange::Bind {
                    command: "layer.new".to_owned(),
                    keys: "Ctrl+J".to_owned()
                },
                KeymapChange::Unbind {
                    keys: "Ctrl+G".to_owned()
                },
                KeymapChange::Clear {
                    command: "edit.undo".to_owned()
                },
                KeymapChange::Reset(Some("edit.undo".to_owned())),
                KeymapChange::Reset(None),
            ]
        );
        let bad = Params::new()
            .with("command", "layer.new")
            .with("keys", "Hyper+J");
        assert!(f.try_run("keymap.bind", bad).is_err());
        assert!(
            f.try_run("keymap.unbind", Params::new().with("keys", ""))
                .is_err()
        );
        assert_eq!(f.state.keymap_changes.len(), 5);
    }

    #[test]
    fn an_aseprite_file_opens_as_a_new_sprite() {
        let mut f = Fixture::new();
        f.run("file.import_aseprite");
        assert_eq!(
            f.state.file_request.take(),
            Some(FileRequest::ImportAseprite)
        );

        f.run_with(
            "file.import_aseprite",
            Params::new().with("path", sample("aseprite/layers.aseprite")),
        );
        assert_eq!(f.state.dialog, None);
        let sprite = f.document().sprite();
        assert_eq!(
            (sprite.width(), sprite.height(), sprite.layer_count()),
            (32, 32, 3)
        );
        // It has no file of its own, and is not waiting to be saved.
        assert_eq!(f.state.file_path, None);
        assert!(!f.state.has_unsaved_changes());
        assert!(f.state.recent.is_empty());

        // Open takes Aseprite files too, and says what did not come across.
        f.run_with(
            "file.open",
            Params::new().with("path", sample("aseprite/combine.aseprite")),
        );
        assert_eq!(f.state.editor.documents().count(), 2);
        let Some(Dialog::Message { title, text }) = f.state.dialog.take() else {
            panic!("no message");
        };
        assert_eq!(title, "Imported with Changes");
        assert!(text.contains("Difference"), "{text}");

        // A file that is not one says so.
        f.run_with(
            "file.open",
            Params::new().with("path", sample("../golden/README.ase")),
        );
        assert!(
            matches!(&f.state.dialog, Some(Dialog::Message { text, .. }) if text.contains("Could not import"))
        );
        assert_eq!(f.state.editor.documents().count(), 2);
    }

    #[test]
    fn a_timelapse_is_exported_from_the_undo_history() {
        let dir = TempDir::new("timelapse");
        let mut f = Fixture::with_sprite();
        // Nothing has been done yet: there is nothing to replay.
        assert!(!f.is_enabled("file.export_timelapse"));
        for row in [2, 5, 8] {
            f.draw_line(row);
        }
        assert!(f.is_enabled("file.export_timelapse"));

        f.run("file.export_timelapse");
        assert_eq!(
            f.state.dialog.take(),
            Some(Dialog::Timelapse {
                scale: 1,
                step_ms: 100
            })
        );
        // With settings, only the file name is still missing.
        let settings = Params::new().with("scale", 2_i64).with("step_ms", 50_i64);
        f.run_with("file.export_timelapse", settings.clone());
        assert!(matches!(
            f.state.file_request.take(),
            Some(FileRequest::Export {
                command: "file.export_timelapse",
                ..
            })
        ));

        let before = (f.document().sprite().clone(), f.document().revision());
        let path = dir.path().join("making-of.gif");
        f.run_with(
            "file.export_timelapse",
            settings.with("path", path.to_string_lossy().into_owned()),
        );
        assert_eq!(f.state.dialog, None);
        let mut decoder = gif::DecodeOptions::new()
            .read_info(std::fs::File::open(&path).unwrap())
            .unwrap();
        assert_eq!((decoder.width(), decoder.height()), (32, 32));
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            delays.push(frame.delay);
        }
        // The blank sprite, then one picture per stroke; the last one is held.
        assert_eq!(delays, [5, 5, 5, 200]);
        // The sprite and its history are as they were.
        assert_eq!(
            (f.document().sprite().clone(), f.document().revision()),
            before
        );
        assert!(f.state.log.last().unwrap().contains("4 pictures"));
    }
}
