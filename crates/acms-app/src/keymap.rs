//! Key chords → command ids, with optional arguments (see `PLAN.md`, section 4.2).
//!
//! The keymap starts from the built-in defaults; the user's keymap file is applied on top.

use acms_core::command::{Params, Value};
use egui::{Key, KeyboardShortcut, Modifiers};
use serde::Deserialize;

/// Chords are written as text (`"Ctrl+Shift+P"`), the same way as in the user's keymap file.
/// `Ctrl` means Cmd on macOS.
const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("Ctrl+N", "file.new"),
    ("Ctrl+O", "file.open"),
    ("Ctrl+S", "file.save"),
    ("Ctrl+Shift+S", "file.save_as"),
    ("Ctrl+E", "file.export_png"),
    ("Ctrl+W", "file.close"),
    ("Ctrl+Tab", "doc.next"),
    ("Ctrl+Shift+Tab", "doc.previous"),
    ("Ctrl+Q", "app.quit"),
    ("Ctrl+X", "edit.cut"),
    ("Ctrl+C", "edit.copy"),
    ("Ctrl+V", "edit.paste"),
    ("Delete", "edit.clear"),
    ("Ctrl+A", "select.all"),
    ("Ctrl+D", "select.none"),
    ("Ctrl+Shift+I", "select.invert"),
    ("Ctrl+Z", "edit.undo"),
    ("Ctrl+Shift+Z", "edit.redo"),
    ("Ctrl+Y", "edit.redo"),
    ("B", "tool.pencil"),
    ("E", "tool.eraser"),
    ("I", "tool.eyedropper"),
    ("L", "tool.line"),
    ("U", "tool.rectangle"),
    ("Shift+U", "tool.ellipse"),
    ("G", "tool.bucket"),
    ("Shift+G", "tool.gradient"),
    ("V", "tool.move"),
    ("M", "tool.select_rectangle"),
    ("Q", "tool.lasso"),
    ("W", "tool.magic_wand"),
    ("X", "color.swap"),
    ("Ctrl+Shift+N", "layer.new"),
    ("Ctrl+B", "brush.from_selection"),
    ("Alt+N", "frame.new"),
    ("Alt+D", "frame.duplicate"),
    ("Period", "frame.next"),
    ("Comma", "frame.previous"),
    ("Home", "frame.first"),
    ("End", "frame.last"),
    ("Enter", "playback.toggle"),
    ("F3", "onion.toggle"),
    ("T", "tool.tile_stamp"),
    ("C", "tool.slice"),
    ("F2", "layer.rename"),
    ("Ctrl+Plus", "view.zoom_in"),
    ("Ctrl+Equals", "view.zoom_in"),
    ("Ctrl+Minus", "view.zoom_out"),
    ("Ctrl+0", "view.zoom_fit"),
    ("Ctrl+G", "view.toggle_grid"),
    ("Ctrl+Shift+P", "view.command_palette"),
    ("Ctrl+K", "view.command_palette"),
    ("Ctrl+Shift+T", "view.toggle_theme"),
    ("Ctrl+Shift+R", "view.reset_layout"),
];

#[cfg(test)]
pub fn default_commands() -> impl Iterator<Item = &'static str> {
    DEFAULT_BINDINGS.iter().map(|(_, command)| *command)
}

struct Binding {
    shortcut: KeyboardShortcut,
    command: String,
    params: Params,
}

/// The user's keymap file. It only holds what differs from the defaults:
///
/// ```json
/// {
///   "bindings": [
///     { "keys": "Ctrl+J", "command": "edit.undo" },
///     { "keys": "Ctrl+1", "command": "brush.set_size", "args": { "size": 1 } },
///     { "keys": "Ctrl+Y", "command": null }
///   ]
/// }
/// ```
///
/// An entry binds its chord to the command, replacing what the chord did before. `args` are
/// passed to the command. A `null` (or missing) command leaves the chord unbound.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeymapFile {
    bindings: Vec<FileBinding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBinding {
    keys: String,
    command: Option<String>,
    #[serde(default)]
    args: serde_json::Map<String, serde_json::Value>,
}

/// Turns the `args` of a keymap entry into command parameters. Lists and nested objects have
/// no meaning for a command.
fn params_from_json(args: serde_json::Map<String, serde_json::Value>) -> Result<Params, String> {
    let mut params = Params::new();
    for (name, value) in args {
        let value = match value {
            serde_json::Value::Bool(flag) => Value::Bool(flag),
            serde_json::Value::String(text) => Value::Text(text),
            serde_json::Value::Number(number) => match (number.as_i64(), number.as_f64()) {
                (Some(whole), _) => Value::Int(whole),
                (None, Some(float)) => Value::Float(float),
                (None, None) => return Err(format!("argument `{name}` is out of range")),
            },
            _ => {
                return Err(format!(
                    "argument `{name}` must be true, false, a number or text"
                ));
            }
        };
        params = params.with(&name, value);
    }
    Ok(params)
}

#[derive(Default)]
pub struct Keymap {
    /// Sorted so chords with more modifiers come first; see [`Keymap::triggered`].
    bindings: Vec<Binding>,
}

impl Keymap {
    pub fn with_defaults() -> Self {
        let mut keymap = Self::default();
        for (chord, command) in DEFAULT_BINDINGS {
            // The table above is covered by a test, so a chord that fails to parse is a typo
            // caught there rather than a silent gap at runtime.
            if let Some(shortcut) = parse_chord(chord) {
                keymap.bind(shortcut, *command, Params::new());
            }
        }
        keymap
    }

    /// Binds `shortcut` to `command`, replacing whatever the chord was bound to before.
    pub fn bind(&mut self, shortcut: KeyboardShortcut, command: impl Into<String>, params: Params) {
        self.unbind(shortcut);
        self.bindings.push(Binding {
            shortcut,
            command: command.into(),
            params,
        });
        self.bindings
            .sort_by_key(|b| std::cmp::Reverse(modifier_count(b.shortcut.modifiers)));
    }

    /// Removes whatever `shortcut` is bound to.
    pub fn unbind(&mut self, shortcut: KeyboardShortcut) {
        self.bindings.retain(|b| b.shortcut != shortcut);
    }

    /// Applies the contents of a keymap file (see [`KeymapFile`]) on top of the current bindings.
    ///
    /// Fails without changing anything if the text is not a keymap file. Entries that cannot
    /// be used are skipped; the returned list has one message for each of them.
    pub fn apply_user_json(&mut self, json: &str) -> Result<Vec<String>, serde_json::Error> {
        let file: KeymapFile = serde_json::from_str(json)?;

        let mut skipped = Vec::new();
        for entry in file.bindings {
            let Some(shortcut) = parse_chord(&entry.keys) else {
                skipped.push(format!("\"{}\" is not a key chord", entry.keys));
                continue;
            };
            let Some(command) = entry.command else {
                self.unbind(shortcut);
                continue;
            };
            match params_from_json(entry.args) {
                Ok(params) => self.bind(shortcut, command, params),
                Err(why) => skipped.push(format!("\"{}\": {why}", entry.keys)),
            }
        }
        Ok(skipped)
    }

    /// The command of every binding; a command bound to two chords appears twice.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.bindings.iter().map(|b| b.command.as_str())
    }

    /// The shortcuts that run a command without arguments, in no particular order. These are
    /// the ones to show next to the command in menus and the palette.
    pub fn shortcuts_for<'a>(
        &'a self,
        command: &'a str,
    ) -> impl Iterator<Item = &'a KeyboardShortcut> {
        self.bindings
            .iter()
            .filter(move |b| b.command == command && b.params.is_empty())
            .map(|b| &b.shortcut)
    }

    /// Returns the command (and its arguments) whose chord was pressed this frame, consuming
    /// the key press.
    pub fn triggered(&self, ctx: &egui::Context) -> Option<(&str, &Params)> {
        // While the user is typing, the keys a text field needs belong to the text field.
        let typing = ctx.text_edit_focused();
        ctx.input_mut(|input| {
            // egui ignores extra Shift/Alt when matching, so `Ctrl+P` would also fire on
            // `Ctrl+Shift+P`. Checking the most specific chords first avoids that.
            self.bindings
                .iter()
                .filter(|b| !(typing && is_text_input(b.shortcut)))
                .find(|b| input.consume_shortcut(&b.shortcut))
                .map(|b| (b.command.as_str(), &b.params))
        })
    }
}

fn modifier_count(modifiers: Modifiers) -> usize {
    [modifiers.command, modifiers.shift, modifiers.alt]
        .into_iter()
        .filter(|&held| held)
        .count()
}

/// Whether a text field would use this chord itself: plain keys for typing, and the usual
/// chords for select all, copy, cut, paste, undo and redo.
fn is_text_input(shortcut: KeyboardShortcut) -> bool {
    let modifiers = shortcut.modifiers;
    if !modifiers.command && !modifiers.alt {
        return true;
    }
    let editing_key = matches!(
        shortcut.logical_key,
        Key::A | Key::C | Key::V | Key::X | Key::Y | Key::Z
    );
    modifiers.command && !modifiers.alt && editing_key
}

/// Parses a chord such as `"Ctrl+Shift+P"`: any number of modifiers, then exactly one key.
pub fn parse_chord(text: &str) -> Option<KeyboardShortcut> {
    let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let key = Key::from_name(parts.pop()?)?;

    let mut modifiers = Modifiers::NONE;
    for part in parts {
        modifiers = modifiers.plus(match part.to_ascii_lowercase().as_str() {
            "ctrl" | "cmd" | "command" => Modifiers::COMMAND,
            "shift" => Modifiers::SHIFT,
            "alt" | "option" => Modifiers::ALT,
            _ => return None,
        });
    }
    Some(KeyboardShortcut::new(modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chords() {
        assert_eq!(
            parse_chord("Ctrl+Shift+P"),
            Some(KeyboardShortcut::new(
                Modifiers::COMMAND.plus(Modifiers::SHIFT),
                Key::P
            ))
        );
        assert_eq!(
            parse_chord(" alt + F4 "),
            Some(KeyboardShortcut::new(Modifiers::ALT, Key::F4))
        );
        assert_eq!(
            parse_chord("Escape"),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::Escape))
        );
    }

    #[test]
    fn rejects_malformed_chords() {
        assert_eq!(parse_chord(""), None);
        assert_eq!(parse_chord("Ctrl+"), None);
        assert_eq!(parse_chord("Hyper+P"), None);
        assert_eq!(parse_chord("Ctrl+NotAKey"), None);
    }

    #[test]
    fn every_default_chord_parses_and_is_used_once() {
        for (chord, _) in DEFAULT_BINDINGS {
            assert!(parse_chord(chord).is_some(), "bad default chord: {chord}");
        }
        // A chord listed twice would silently lose its first command.
        assert_eq!(
            Keymap::with_defaults().bindings.len(),
            DEFAULT_BINDINGS.len()
        );
    }

    fn chords_for(keymap: &Keymap, command: &str) -> Vec<KeyboardShortcut> {
        keymap.shortcuts_for(command).copied().collect()
    }

    #[test]
    fn rebinding_a_chord_replaces_the_old_command() {
        let chord = parse_chord("Ctrl+G").unwrap();
        let mut keymap = Keymap::with_defaults();
        keymap.bind(chord, "view.toggle_theme", Params::new());
        assert!(chords_for(&keymap, "view.toggle_grid").is_empty());
        assert!(chords_for(&keymap, "view.toggle_theme").contains(&chord));
    }

    #[test]
    fn user_file_rebinds_and_unbinds_on_top_of_the_defaults() {
        let mut keymap = Keymap::with_defaults();
        let skipped = keymap
            .apply_user_json(
                r#"{ "bindings": [
                    { "keys": "Ctrl+J", "command": "view.toggle_grid" },
                    { "keys": "Ctrl+G", "command": null },
                    { "keys": "Ctrl+Q", "command": "view.toggle_theme" }
                ] }"#,
            )
            .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(
            chords_for(&keymap, "view.toggle_grid"),
            [parse_chord("Ctrl+J").unwrap()]
        );
        assert!(chords_for(&keymap, "app.quit").is_empty());
        assert!(chords_for(&keymap, "view.toggle_theme").contains(&parse_chord("Ctrl+Q").unwrap()));
        // Defaults the file does not mention are untouched.
        assert_eq!(chords_for(&keymap, "view.command_palette").len(), 2);
    }

    #[test]
    fn user_file_bindings_can_carry_arguments() {
        let mut keymap = Keymap::default();
        let skipped = keymap
            .apply_user_json(
                r##"{ "bindings": [
                    { "keys": "Ctrl+1", "command": "brush.set_size", "args": { "size": 1 } },
                    { "keys": "Ctrl+2", "command": "x", "args": { "a": true, "b": 0.5, "c": "#fff" } },
                    { "keys": "Ctrl+3", "command": "x", "args": { "list": [1, 2] } }
                ] }"##,
            )
            .unwrap();

        assert_eq!(
            skipped,
            ["\"Ctrl+3\": argument `list` must be true, false, a number or text"]
        );
        let binding = |command: &str| {
            keymap
                .bindings
                .iter()
                .find(|b| b.command == command)
                .map(|b| b.params.clone())
        };
        assert_eq!(
            binding("brush.set_size"),
            Some(Params::new().with("size", 1_i64))
        );
        assert_eq!(
            binding("x"),
            Some(
                Params::new()
                    .with("a", true)
                    .with("b", 0.5)
                    .with("c", "#fff")
            )
        );
        // A binding with arguments is a different action from the bare command, so it is not
        // shown as that command's shortcut.
        assert!(chords_for(&keymap, "brush.set_size").is_empty());
    }

    #[test]
    fn user_file_entries_with_a_bad_chord_are_skipped_and_reported() {
        let mut keymap = Keymap::with_defaults();
        let skipped = keymap
            .apply_user_json(
                r#"{ "bindings": [
                    { "keys": "Hyper+J", "command": "edit.undo" },
                    { "keys": "Ctrl+J", "command": "edit.undo" }
                ] }"#,
            )
            .unwrap();

        assert_eq!(skipped, ["\"Hyper+J\" is not a key chord"]);
        assert!(chords_for(&keymap, "edit.undo").contains(&parse_chord("Ctrl+J").unwrap()));
    }

    #[test]
    fn a_broken_user_file_changes_nothing() {
        for broken in [
            "not json",
            r#"{ "bindings": "Ctrl+J" }"#,
            r#"{ "binding": [] }"#,
            r#"{ "bindings": [ { "key": "Ctrl+J", "command": "app.quit" } ] }"#,
            // Valid first entry, broken second: the first must not be applied either.
            r#"{ "bindings": [ { "keys": "Ctrl+J", "command": "app.quit" }, 5 ] }"#,
        ] {
            let mut keymap = Keymap::with_defaults();
            assert!(
                keymap.apply_user_json(broken).is_err(),
                "accepted: {broken}"
            );
            assert_eq!(keymap.bindings.len(), DEFAULT_BINDINGS.len());
            assert_eq!(
                chords_for(&keymap, "app.quit"),
                [parse_chord("Ctrl+Q").unwrap()]
            );
        }
    }

    #[test]
    fn more_specific_chords_are_checked_first() {
        let mut keymap = Keymap::default();
        keymap.bind(parse_chord("Ctrl+P").unwrap(), "a", Params::new());
        keymap.bind(parse_chord("Ctrl+Shift+P").unwrap(), "b", Params::new());
        let order: Vec<&str> = keymap.bindings.iter().map(|b| b.command.as_str()).collect();
        assert_eq!(order, ["b", "a"]);
    }

    #[test]
    fn text_fields_keep_the_keys_they_need() {
        let needed = [
            "B",
            "Shift+B",
            "Ctrl+Z",
            "Ctrl+Shift+Z",
            "Ctrl+Y",
            "Ctrl+A",
            "Ctrl+V",
        ];
        let free = [
            "Ctrl+S",
            "Ctrl+Shift+P",
            "Ctrl+K",
            "Ctrl+N",
            "Alt+Z",
            "Ctrl+Alt+Z",
        ];
        for chord in needed {
            assert!(is_text_input(parse_chord(chord).unwrap()), "{chord}");
        }
        for chord in free {
            assert!(!is_text_input(parse_chord(chord).unwrap()), "{chord}");
        }
    }
}
