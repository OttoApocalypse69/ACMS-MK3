//! Key chords → command ids (see `PLAN.md`, section 4.2).
//!
//! The keymap starts from the built-in defaults; the user's keymap file is applied on top.

use egui::{Key, KeyboardShortcut, Modifiers};
use serde::Deserialize;

/// Chords are written as text (`"Ctrl+Shift+P"`), the same way as in the user's keymap file.
/// `Ctrl` means Cmd on macOS.
const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("Ctrl+Shift+P", "view.command_palette"),
    ("Ctrl+K", "view.command_palette"),
    ("Ctrl+Shift+T", "view.toggle_theme"),
    ("Ctrl+Shift+R", "view.reset_layout"),
    ("Ctrl+H", "debug.say_hello"),
    ("Ctrl+Q", "app.quit"),
];

#[cfg(test)]
pub fn default_commands() -> impl Iterator<Item = &'static str> {
    DEFAULT_BINDINGS.iter().map(|(_, command)| *command)
}

struct Binding {
    shortcut: KeyboardShortcut,
    command: String,
}

/// The user's keymap file. It only holds what differs from the defaults:
///
/// ```json
/// {
///   "bindings": [
///     { "keys": "Ctrl+J", "command": "debug.say_hello" },
///     { "keys": "Ctrl+H", "command": null }
///   ]
/// }
/// ```
///
/// An entry binds its chord to the command, replacing what the chord did before. A `null`
/// (or missing) command leaves the chord unbound.
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
                keymap.bind(shortcut, *command);
            }
        }
        keymap
    }

    /// Binds `shortcut` to `command`, replacing whatever the chord was bound to before.
    pub fn bind(&mut self, shortcut: KeyboardShortcut, command: impl Into<String>) {
        self.bindings.retain(|b| b.shortcut != shortcut);
        self.bindings.push(Binding {
            shortcut,
            command: command.into(),
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
    /// Fails without changing anything if the text is not a keymap file. Entries whose chord
    /// cannot be parsed are skipped; the returned list has one message for each of them.
    pub fn apply_user_json(&mut self, json: &str) -> Result<Vec<String>, serde_json::Error> {
        let file: KeymapFile = serde_json::from_str(json)?;

        let mut skipped = Vec::new();
        for entry in file.bindings {
            let Some(shortcut) = parse_chord(&entry.keys) else {
                skipped.push(format!("\"{}\" is not a key chord", entry.keys));
                continue;
            };
            match entry.command {
                Some(command) => self.bind(shortcut, command),
                None => self.unbind(shortcut),
            }
        }
        Ok(skipped)
    }

    /// The command of every binding; a command bound to two chords appears twice.
    pub fn commands(&self) -> impl Iterator<Item = &str> {
        self.bindings.iter().map(|b| b.command.as_str())
    }

    /// The shortcuts bound to a command, in no particular order.
    pub fn shortcuts_for<'a>(
        &'a self,
        command: &'a str,
    ) -> impl Iterator<Item = &'a KeyboardShortcut> {
        self.bindings
            .iter()
            .filter(move |b| b.command == command)
            .map(|b| &b.shortcut)
    }

    /// Returns the command whose chord was pressed this frame, consuming the key press.
    pub fn triggered(&self, ctx: &egui::Context) -> Option<&str> {
        // While the user is typing, plain keys belong to the text field.
        let typing = ctx.text_edit_focused();
        ctx.input_mut(|input| {
            // egui ignores extra Shift/Alt when matching, so `Ctrl+P` would also fire on
            // `Ctrl+Shift+P`. Checking the most specific chords first avoids that.
            self.bindings
                .iter()
                .filter(|b| !(typing && is_plain_key(b.shortcut.modifiers)))
                .find(|b| input.consume_shortcut(&b.shortcut))
                .map(|b| b.command.as_str())
        })
    }
}

fn modifier_count(modifiers: Modifiers) -> usize {
    [modifiers.command, modifiers.shift, modifiers.alt]
        .into_iter()
        .filter(|&held| held)
        .count()
}

fn is_plain_key(modifiers: Modifiers) -> bool {
    !modifiers.command && !modifiers.alt
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
    fn every_default_chord_parses() {
        for (chord, _) in DEFAULT_BINDINGS {
            assert!(parse_chord(chord).is_some(), "bad default chord: {chord}");
        }
        assert_eq!(
            Keymap::with_defaults().bindings.len(),
            DEFAULT_BINDINGS.len()
        );
    }

    #[test]
    fn rebinding_a_chord_replaces_the_old_command() {
        let chord = parse_chord("Ctrl+H").unwrap();
        let mut keymap = Keymap::with_defaults();
        keymap.bind(chord, "view.toggle_theme");
        assert_eq!(keymap.shortcuts_for("debug.say_hello").count(), 0);
        assert!(
            keymap
                .shortcuts_for("view.toggle_theme")
                .any(|s| *s == chord)
        );
    }

    fn chords_for(keymap: &Keymap, command: &str) -> Vec<KeyboardShortcut> {
        keymap.shortcuts_for(command).copied().collect()
    }

    #[test]
    fn user_file_rebinds_and_unbinds_on_top_of_the_defaults() {
        let mut keymap = Keymap::with_defaults();
        let skipped = keymap
            .apply_user_json(
                r#"{ "bindings": [
                    { "keys": "Ctrl+J", "command": "debug.say_hello" },
                    { "keys": "Ctrl+H", "command": null },
                    { "keys": "Ctrl+Q", "command": "view.toggle_theme" }
                ] }"#,
            )
            .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(
            chords_for(&keymap, "debug.say_hello"),
            [parse_chord("Ctrl+J").unwrap()]
        );
        assert!(chords_for(&keymap, "app.quit").is_empty());
        assert!(chords_for(&keymap, "view.toggle_theme").contains(&parse_chord("Ctrl+Q").unwrap()));
        // Defaults the file does not mention are untouched.
        assert_eq!(chords_for(&keymap, "view.command_palette").len(), 2);
    }

    #[test]
    fn user_file_entries_with_a_bad_chord_are_skipped_and_reported() {
        let mut keymap = Keymap::with_defaults();
        let skipped = keymap
            .apply_user_json(
                r#"{ "bindings": [
                    { "keys": "Hyper+J", "command": "debug.say_hello" },
                    { "keys": "Ctrl+J", "command": "debug.say_hello" }
                ] }"#,
            )
            .unwrap();

        assert_eq!(skipped, ["\"Hyper+J\" is not a key chord"]);
        assert!(chords_for(&keymap, "debug.say_hello").contains(&parse_chord("Ctrl+J").unwrap()));
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
        keymap.bind(parse_chord("Ctrl+P").unwrap(), "a");
        keymap.bind(parse_chord("Ctrl+Shift+P").unwrap(), "b");
        let order: Vec<&str> = keymap.bindings.iter().map(|b| b.command.as_str()).collect();
        assert_eq!(order, ["b", "a"]);
    }
}
