//! Per-user configuration files.

use std::io;
use std::path::{Path, PathBuf};

use crate::keymap::Keymap;

/// Where the user's keymap file lives: `%APPDATA%\acms-mk3\config\keymap.json` on Windows,
/// `~/.config/acms-mk3/keymap.json` on Linux.
///
/// `None` if the OS cannot tell us where the user's home is.
pub fn keymap_path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "acms-mk3")?;
    Some(dirs.config_dir().join("keymap.json"))
}

/// Where the user's own scripts live: a `scripts` folder next to the keymap file.
pub fn scripts_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "acms-mk3")?;
    Some(dirs.config_dir().join("scripts"))
}

/// Where sprites with unsaved changes are autosaved: a `recovery` folder in the app's own
/// data folder, which unlike the configuration is not meant to be carried between machines.
pub fn recovery_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "acms-mk3")?;
    Some(dirs.data_local_dir().join("recovery"))
}

/// Builds the keymap: the built-in defaults with the file at `path` applied on top.
///
/// A missing or broken file is never fatal: the defaults are used instead. The returned lines
/// say what happened and are meant for the Log panel.
pub fn load_keymap(path: &Path) -> (Keymap, Vec<String>) {
    let mut keymap = Keymap::with_defaults();
    let mut messages = Vec::new();
    let shown = path.display();

    match std::fs::read_to_string(path) {
        Ok(json) => match keymap.apply_user_json(&json) {
            Ok(skipped) => {
                messages.push(format!("Keymap: loaded {shown}"));
                messages.extend(skipped.into_iter().map(|why| format!("Keymap: {why}")));
            }
            Err(err) => messages.push(format!(
                "Keymap: {shown} is not a valid keymap file, using the defaults ({err})"
            )),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            messages.push(format!("Keymap: using the defaults (no file at {shown})"));
        }
        Err(err) => messages.push(format!(
            "Keymap: could not read {shown}, using the defaults ({err})"
        )),
    }

    (keymap, messages)
}

/// A file in the OS temp directory that is deleted when dropped.
#[cfg(test)]
pub struct TempFile(PathBuf);

#[cfg(test)]
impl TempFile {
    /// `name` must be unique among the tests, which run in parallel.
    pub fn new(name: &str, contents: &str) -> Self {
        let file = Self::path_only(name);
        std::fs::write(file.path(), contents).unwrap();
        file
    }

    /// Reserves a path without creating the file, for tests that write it themselves.
    pub fn path_only(name: &str) -> Self {
        let file_name = format!("acms-test-{}-{name}", std::process::id());
        let path = std::env::temp_dir().join(file_name);
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A folder in the OS temp directory that is deleted, with what is in it, when dropped.
#[cfg(test)]
pub struct TempDir(PathBuf);

#[cfg(test)]
impl TempDir {
    /// `name` must be unique among the tests, which run in parallel.
    pub fn new(name: &str) -> Self {
        let folder = format!("acms-test-{}-{name}", std::process::id());
        let path = std::env::temp_dir().join(folder);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::parse_chord;

    fn has(keymap: &Keymap, command: &str, chord: &str) -> bool {
        let chord = parse_chord(chord).unwrap();
        keymap.shortcuts_for(command).any(|s| *s == chord)
    }

    #[test]
    fn keymap_file_sits_in_the_apps_own_config_folder() {
        // `None` is fine on a machine without a home directory; a wrong folder is not.
        if let Some(path) = keymap_path() {
            assert!(path.ends_with("keymap.json"));
            assert!(path.to_string_lossy().contains("acms-mk3"));
        }
    }

    #[test]
    fn a_missing_file_means_defaults() {
        let path = std::env::temp_dir().join("acms-test-this-file-does-not-exist.json");
        let (keymap, messages) = load_keymap(&path);
        assert!(has(&keymap, "view.toggle_grid", "Ctrl+G"));
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("using the defaults"));
    }

    #[test]
    fn a_file_is_applied_on_top_of_the_defaults() {
        let file = TempFile::new(
            "config-applied.json",
            r#"{ "bindings": [
                { "keys": "Ctrl+J", "command": "view.toggle_grid" },
                { "keys": "Nope", "command": "view.toggle_grid" }
            ] }"#,
        );
        let (keymap, messages) = load_keymap(file.path());
        assert!(has(&keymap, "view.toggle_grid", "Ctrl+J"));
        assert!(has(&keymap, "view.toggle_grid", "Ctrl+G"));
        assert_eq!(messages.len(), 2);
        assert!(messages[0].starts_with("Keymap: loaded "));
        assert_eq!(messages[1], "Keymap: \"Nope\" is not a key chord");
    }

    #[test]
    fn a_broken_file_means_defaults_and_a_message() {
        let file = TempFile::new("config-broken.json", "{ this is not json");
        let (keymap, messages) = load_keymap(file.path());
        assert!(has(&keymap, "view.toggle_grid", "Ctrl+G"));
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("is not a valid keymap file"));
    }
}
