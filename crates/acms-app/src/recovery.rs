//! Autosave and crash recovery.
//!
//! Every few minutes each sprite with unsaved changes is written to the recovery folder, next
//! to a small note saying which file it belongs to. The copies are removed when the sprite is
//! saved or closed, and when the app exits normally. Copies found at start mean the app did
//! not exit normally last time: the user is asked whether to open them.
//!
//! Each running app holds a lock on a file of its own in the recovery folder, so that the
//! copies of an app that is still running are never taken for those of one that crashed.

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind};
use acms_core::document::Document;
use acms_core::editor::DocId;
use acms_io::acms::{load_acms, save_acms};
use serde::{Deserialize, Serialize};

use crate::commands::sentence;
use crate::settings;
use crate::state::{AppState, Dialog};

/// What is written next to an autosaved sprite.
#[derive(Serialize, Deserialize)]
struct Note {
    /// The file the sprite was opened from or last saved to, if it has one.
    path: Option<PathBuf>,
}

/// An autosaved sprite left behind by a run that did not end normally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leftover {
    /// The copy in the recovery folder.
    pub copy: PathBuf,
    /// The file the sprite belongs to, if it had been saved before.
    pub original: Option<PathBuf>,
}

impl Leftover {
    /// The file a recovered sprite is saved to. The note saying so is only trusted as far as
    /// naming a sprite file goes: anything else, and the sprite comes back without a file.
    pub fn target(&self) -> Option<&Path> {
        self.original.as_deref().filter(|path| {
            path.is_absolute()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("acms"))
        })
    }

    /// What to call the sprite when asking about it.
    pub fn title(&self) -> String {
        self.original
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || "Untitled".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
    }
}

#[derive(Default)]
pub struct RecoveryState {
    /// For each sprite with a copy in the recovery folder: the revision the copy is of.
    saved: HashMap<DocId, u64>,
    /// When sprites were last autosaved, in seconds of the app's clock.
    last: Option<f64>,
    /// Copies being written right now, by one thread per round.
    writing: Vec<JoinHandle<Vec<String>>>,
    /// What was found at start, until the user has decided about it.
    pub found: Vec<Leftover>,
    /// The file this app holds a lock on while it runs, and the lock.
    lock: Option<(PathBuf, File)>,
}

fn copy_path(folder: &Path, id: DocId) -> PathBuf {
    folder.join(format!("{}-{}.acms", std::process::id(), id.0))
}

fn lock_path(folder: &Path, pid: u32) -> PathBuf {
    folder.join(format!("{pid}.lock"))
}

/// Takes the lock that says this app is running. Without it, nothing is lost: another app
/// may only take this one's copies for leftovers.
fn take_lock(folder: &Path) -> Option<(PathBuf, File)> {
    std::fs::create_dir_all(folder).ok()?;
    let path = lock_path(folder, std::process::id());
    let file = File::create(&path).ok()?;
    file.try_lock().ok()?;
    Some((path, file))
}

/// Whether the app that wrote copies under `pid` is still running: it holds its lock.
fn still_running(folder: &Path, pid: u32) -> bool {
    if pid == std::process::id() {
        // This app has not written anything yet: an earlier one with the same number did.
        return false;
    }
    let path = lock_path(folder, pid);
    let Ok(file) = File::options().write(true).open(&path) else {
        return false;
    };
    match file.try_lock() {
        Err(TryLockError::WouldBlock) => true,
        Ok(()) | Err(TryLockError::Error(_)) => {
            // Nobody holds it: what is left of a run that ended.
            drop(file);
            let _ = std::fs::remove_file(&path);
            false
        }
    }
}

/// The number of the app that wrote a copy, from its name `<pid>-<sprite>.acms`.
fn writer_of(copy: &Path) -> Option<u32> {
    let stem = copy.file_stem()?.to_str()?;
    stem.split_once('-')?.0.parse().ok()
}

fn note_path(copy: &Path) -> PathBuf {
    copy.with_extension("json")
}

fn remove(copy: &Path) {
    let _ = std::fs::remove_file(copy);
    let _ = std::fs::remove_file(note_path(copy));
}

/// Collects what finished writing, logging what went wrong. Returns whether copies are still
/// being written.
fn settle(state: &mut AppState, wait: bool) -> bool {
    let mut pending = Vec::new();
    for handle in std::mem::take(&mut state.recovery.writing) {
        if wait || handle.is_finished() {
            for problem in handle.join().unwrap_or_default() {
                state.log.push(format!("Autosave: {problem}"));
            }
        } else {
            pending.push(handle);
        }
    }
    state.recovery.writing = pending;
    !state.recovery.writing.is_empty()
}

/// Writes a copy of every sprite that has unsaved changes the last copy does not have, and
/// removes the copies of sprites that no longer need one. The writing happens on another
/// thread, so drawing is not held up; `wait` holds on until it is done.
pub fn autosave(state: &mut AppState, wait: bool) {
    let Some(folder) = state.paths.recovery.clone() else {
        return;
    };
    // The copies of the last round are still being written: this round is skipped.
    if settle(state, wait) {
        return;
    }

    let open: Vec<DocId> = state.editor.documents().map(|(id, _)| id).collect();
    let stale: Vec<DocId> = state
        .recovery
        .saved
        .keys()
        .copied()
        .filter(|id| !open.contains(id) || !state.is_unsaved(*id))
        .collect();
    for id in stale {
        state.recovery.saved.remove(&id);
        remove(&copy_path(&folder, id));
    }

    // One thread writes the whole round, one copy after another, however many sprites
    // are open.
    let mut jobs = Vec::new();
    for id in open {
        let Some(document) = state.editor.document_by_id(id) else {
            continue;
        };
        let revision = document.revision();
        if !state.is_unsaved(id) || state.recovery.saved.get(&id) == Some(&revision) {
            continue;
        }
        // Cloning a sprite shares its pictures; the copy costs next to nothing.
        let sprite = document.sprite().clone();
        let note = Note {
            path: state.path_of(id).cloned(),
        };
        state.recovery.saved.insert(id, revision);
        jobs.push((copy_path(&folder, id), sprite, note));
    }
    if !jobs.is_empty() {
        state.recovery.writing.push(std::thread::spawn(move || {
            let mut problems = Vec::new();
            for (copy, sprite, note) in jobs {
                let write = || -> Result<(), String> {
                    std::fs::create_dir_all(&folder).map_err(|err| err.to_string())?;
                    save_acms(&sprite, &copy).map_err(|err| err.to_string())?;
                    let note = serde_json::to_string(&note).map_err(|err| err.to_string())?;
                    settings::write_text(&note_path(&copy), &note)
                };
                if let Err(why) = write() {
                    problems.push(format!("could not write {} ({why})", copy.display()));
                }
            }
            problems
        }));
    }
    settle(state, wait);
}

/// Autosaves when the interval set in the preferences has passed. `now` is the app's clock,
/// in seconds. Returns how many seconds to wait before the next call is worth making, or
/// `None` if autosave is off or nothing needs saving.
pub fn tick(state: &mut AppState, now: f64) -> Option<f64> {
    settle(state, false);
    let minutes = state.prefs.autosave_minutes;
    if minutes == 0 || state.paths.recovery.is_none() {
        return None;
    }
    let interval = f64::from(minutes) * 60.0;
    let last = *state.recovery.last.get_or_insert(now);
    if now - last >= interval {
        state.recovery.last = Some(now);
        autosave(state, false);
    }
    let due = state
        .recovery
        .last
        .map_or(interval, |last| last + interval - now);
    (state.any_unsaved_changes() || !state.recovery.saved.is_empty()).then_some(due.max(1.0))
}

/// The autosaved sprites in the recovery folder.
fn leftovers(folder: &Path) -> Vec<Leftover> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut found: Vec<Leftover> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "acms")
        })
        // The copies of an app that is running now are its own business.
        .filter(|copy| !writer_of(copy).is_some_and(|pid| still_running(folder, pid)))
        .map(|copy| {
            let note = std::fs::read_to_string(note_path(&copy))
                .ok()
                .and_then(|text| serde_json::from_str::<Note>(&text).ok());
            Leftover {
                original: note.and_then(|note| note.path),
                copy,
            }
        })
        .collect();
    found.sort_by(|a, b| a.copy.cmp(&b.copy));
    found
}

/// Looks for sprites left behind by a run that did not end normally, and asks about them.
/// Called once, at start.
pub fn check_at_start(state: &mut AppState) {
    let Some(folder) = state.paths.recovery.clone() else {
        return;
    };
    state.recovery.lock = take_lock(&folder);
    state.recovery.found = leftovers(&folder);
    if !state.recovery.found.is_empty() {
        state.dialog = Some(Dialog::Recover);
    }
}

/// Removes this run's copies: the app is closing normally, so nothing needs recovering.
pub fn clean_exit(state: &mut AppState) {
    settle(state, true);
    let Some(folder) = state.paths.recovery.clone() else {
        return;
    };
    for (id, _) in state.recovery.saved.drain() {
        remove(&copy_path(&folder, id));
    }
    if let Some((path, file)) = state.recovery.lock.take() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
}

fn recover(state: &mut AppState, discard: bool) -> Result<(), CommandError> {
    let found = std::mem::take(&mut state.recovery.found);
    let mut problems = Vec::new();
    for leftover in found {
        if !discard {
            match load_acms(&leftover.copy) {
                Ok(sprite) => {
                    let target = leftover.target().map(Path::to_path_buf);
                    state.open_document(Document::new(sprite), target);
                    // It differs from what is on disk, whatever its history says.
                    state.saved_revision = u64::MAX;
                    state.log.push(format!("Recovered {}", leftover.title()));
                }
                Err(err) => {
                    // The copy stays where it is, for another try or a look by hand.
                    problems.push(format!("{}: {err}", leftover.copy.display()));
                    continue;
                }
            }
        }
        remove(&leftover.copy);
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(CommandError::failed(sentence(format!(
            "could not recover {}",
            problems.join("; ")
        ))))
    }
}

pub fn register_commands(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    registry.register(
        Command::new(
            "file.recover",
            "Recover Unsaved Sprites",
            |state: &mut AppState, params| recover(state, params.bool("discard") == Some(true)),
        )
        .optional_param("discard", ParamKind::Bool)
        .enabled_when(|state| !state.recovery.found.is_empty()),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use acms_core::command::Params;

    use super::*;
    use crate::commands::testing::{Fixture, RED};
    use crate::config::TempDir;
    use crate::settings::Paths;

    fn fixture_in(dir: &TempDir) -> Fixture {
        let mut f = Fixture::new();
        f.state.paths = Paths::in_dir(dir.path());
        check_at_start(&mut f.state);
        f
    }

    fn new_sprite(f: &mut Fixture) {
        let size = Params::new().with("width", 16_u32).with("height", 16_u32);
        f.run_with("file.new", size);
    }

    fn copies(dir: &TempDir) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir.path().join("recovery")) else {
            return Vec::new();
        };
        // The lock of the app that is running is not a copy.
        let mut names: Vec<String> = entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.ends_with(".lock"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn sprites_with_unsaved_changes_are_copied_and_the_copies_follow_them() {
        let dir = TempDir::new("autosave");
        let mut f = fixture_in(&dir);
        assert_eq!(f.state.dialog, None);
        new_sprite(&mut f);
        // Nothing has changed yet: nothing to keep.
        autosave(&mut f.state, true);
        assert!(copies(&dir).is_empty());

        f.draw_line(3);
        autosave(&mut f.state, true);
        let first = copies(&dir);
        assert_eq!(first.len(), 2, "{first:?}");
        assert!(first[0].ends_with(".acms") && first[1].ends_with(".json"));
        let copy = dir.path().join("recovery").join(&first[0]);
        assert_eq!(load_acms(&copy).unwrap(), *f.document().sprite());
        // No change, no new copy; a change, and the copy has it.
        let written = std::fs::metadata(&copy).unwrap().modified().unwrap();
        autosave(&mut f.state, true);
        assert_eq!(
            std::fs::metadata(&copy).unwrap().modified().unwrap(),
            written
        );
        f.draw_line(6);
        autosave(&mut f.state, true);
        assert_eq!(load_acms(&copy).unwrap(), *f.document().sprite());

        // A second sprite gets a copy of its own.
        new_sprite(&mut f);
        f.draw_line(1);
        autosave(&mut f.state, true);
        assert_eq!(copies(&dir).len(), 4);

        // Saved: its copy goes. Closed without saving: so does the other.
        let saved = dir.path().join("mine.acms");
        f.run_with(
            "file.save_as",
            Params::new().with("path", saved.to_string_lossy().into_owned()),
        );
        autosave(&mut f.state, true);
        assert_eq!(copies(&dir), first);
        f.run("file.close");
        f.run_with("file.close", Params::new().with("discard", true));
        autosave(&mut f.state, true);
        assert!(copies(&dir).is_empty());
        assert!(
            f.state.log.iter().all(|line| !line.contains("Autosave")),
            "{:?}",
            f.state.log
        );
    }

    #[test]
    fn autosave_follows_the_interval_in_the_preferences() {
        let dir = TempDir::new("autosave-tick");
        let mut f = fixture_in(&dir);
        f.state.prefs.autosave_minutes = 2;
        new_sprite(&mut f);
        // With nothing unsaved there is no reason to be woken.
        assert_eq!(tick(&mut f.state, 0.0), None);
        f.draw_line(3);
        // The clock starts at the first look; nothing is written before the interval is up.
        assert_eq!(tick(&mut f.state, 10.0), Some(110.0));
        assert_eq!(tick(&mut f.state, 119.0), Some(1.0));
        assert!(copies(&dir).is_empty());
        assert_eq!(tick(&mut f.state, 120.5), Some(120.0));
        settle(&mut f.state, true);
        assert_eq!(copies(&dir).len(), 2);

        // Switched off, nothing happens however long it takes.
        f.state.prefs.autosave_minutes = 0;
        f.draw_line(5);
        assert_eq!(tick(&mut f.state, 100_000.0), None);
        let copy = dir.path().join("recovery").join(&copies(&dir)[0]);
        assert_ne!(load_acms(&copy).unwrap(), *f.document().sprite());
    }

    #[test]
    fn a_normal_exit_leaves_nothing_behind() {
        let dir = TempDir::new("autosave-exit");
        let mut f = fixture_in(&dir);
        new_sprite(&mut f);
        f.draw_line(3);
        autosave(&mut f.state, false);
        clean_exit(&mut f.state);
        assert!(copies(&dir).is_empty());
        assert_eq!(fixture_in(&dir).state.dialog, None);
    }

    /// Leaves two autosaved sprites behind, as a crash would: one that had a file, one new.
    fn crashed(dir: &TempDir) -> std::path::PathBuf {
        let mut f = fixture_in(dir);
        let saved = dir.path().join("hero.acms");
        new_sprite(&mut f);
        f.run_with(
            "file.save_as",
            Params::new().with("path", saved.to_string_lossy().into_owned()),
        );
        f.draw_line(3);
        new_sprite(&mut f);
        f.draw_line(9);
        autosave(&mut f.state, true);
        saved
    }

    #[test]
    fn after_a_crash_the_sprites_are_offered_and_recovered() {
        let dir = TempDir::new("recover");
        let saved = crashed(&dir);

        let mut f = fixture_in(&dir);
        assert_eq!(f.state.dialog.take(), Some(Dialog::Recover));
        let titles: Vec<String> = f.state.recovery.found.iter().map(Leftover::title).collect();
        assert_eq!(titles, ["hero.acms", "Untitled"]);

        f.run("file.recover");
        assert_eq!(f.state.dialog, None);
        assert_eq!(f.state.editor.documents().count(), 2);
        // Both are back with what was drawn, and both count as unsaved.
        assert!(f.state.any_unsaved_changes());
        let ids: Vec<_> = f.state.editor.documents().map(|(id, _)| id).collect();
        assert!(ids.iter().all(|id| f.state.is_unsaved(*id)));
        assert_eq!(f.state.path_of(ids[0]), Some(&saved));
        assert_eq!(f.state.path_of(ids[1]), None);
        let first = f.state.editor.document_by_id(ids[0]).unwrap().sprite();
        assert_eq!(first.pixel(acms_core::sprite::LayerId(1), 0, 5, 3), RED);
        assert_eq!(f.pixel(5, 9), RED);
        // The file on disk still has the old picture: recovering changed nothing there.
        assert_ne!(load_acms(&saved).unwrap(), *first);
        // The copies are gone, and the question is not asked again.
        assert!(copies(&dir).is_empty());
        assert!(!f.is_enabled("file.recover"));
        assert_eq!(fixture_in(&dir).state.dialog, None);

        // Saving a recovered sprite goes to its own file, and then it is saved.
        f.run_with("doc.activate", Params::new().with("doc", ids[0].0 as i64));
        f.run("file.save");
        assert!(!f.state.has_unsaved_changes());
        assert_eq!(load_acms(&saved).unwrap(), *f.document().sprite());
    }

    #[test]
    fn recovered_sprites_can_be_discarded_or_left_for_later() {
        let dir = TempDir::new("recover-discard");
        crashed(&dir);
        // Later: the dialog is closed, the copies stay, and the question comes again.
        let mut f = fixture_in(&dir);
        f.state.dialog = None;
        assert_eq!(copies(&dir).len(), 4);
        assert!(f.is_enabled("file.recover"));
        drop(f);

        let mut f = fixture_in(&dir);
        assert_eq!(f.state.dialog.take(), Some(Dialog::Recover));
        f.run_with("file.recover", Params::new().with("discard", true));
        assert_eq!(f.state.editor.documents().count(), 0);
        assert!(copies(&dir).is_empty());
    }

    #[test]
    fn a_damaged_copy_is_reported_and_kept() {
        let dir = TempDir::new("recover-damaged");
        let folder = dir.path().join("recovery");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("1-1.acms"), "not a sprite").unwrap();
        let mut f = fixture_in(&dir);
        assert_eq!(f.state.dialog.take(), Some(Dialog::Recover));
        // No note next to it: it is offered all the same, as a sprite without a file.
        assert_eq!(f.state.recovery.found[0].title(), "Untitled");
        f.run("file.recover");
        assert!(
            matches!(&f.state.dialog, Some(Dialog::Message { text, .. }) if text.contains("Could not recover")),
            "{:?}",
            f.state.dialog
        );
        assert_eq!(copies(&dir), ["1-1.acms"]);
        assert_eq!(f.state.editor.documents().count(), 0);
    }

    #[test]
    fn the_copies_of_an_app_that_is_still_running_are_left_alone() {
        let dir = TempDir::new("recover-running");
        let folder = dir.path().join("recovery");
        std::fs::create_dir_all(&folder).unwrap();
        // Another app, number 1, is running: it holds its lock.
        let other = File::create(lock_path(&folder, 1)).unwrap();
        other.try_lock().unwrap();
        std::fs::write(folder.join("1-1.acms"), "a sprite being worked on").unwrap();
        // App number 2 crashed: its lock is there, but nobody holds it.
        File::create(lock_path(&folder, 2)).unwrap();
        std::fs::write(folder.join("2-1.acms"), "a sprite left behind").unwrap();

        let mut f = fixture_in(&dir);
        let found: Vec<_> = f
            .state
            .recovery
            .found
            .iter()
            .map(|l| l.copy.clone())
            .collect();
        assert_eq!(found, [folder.join("2-1.acms")]);
        // Discarding what was left behind does not touch the running app's copy.
        f.run_with("file.recover", Params::new().with("discard", true));
        assert!(folder.join("1-1.acms").exists());
        assert!(!folder.join("2-1.acms").exists());
        assert!(!lock_path(&folder, 2).exists());
        drop(other);
    }

    #[test]
    fn a_recovered_sprite_is_only_saved_where_a_sprite_file_was() {
        let leftover = |original: &str| Leftover {
            copy: PathBuf::from("1-1.acms"),
            original: Some(PathBuf::from(original)),
        };
        let absolute = std::env::temp_dir().join("hero.acms");
        let named = absolute.to_string_lossy().into_owned();
        assert_eq!(leftover(&named).target(), Some(absolute.as_path()));
        let elsewhere = std::env::temp_dir().join("notes.txt");
        assert_eq!(leftover(&elsewhere.to_string_lossy()).target(), None);
        assert_eq!(leftover("relative.acms").target(), None);
    }

    #[test]
    fn many_sprites_are_written_by_one_thread() {
        let dir = TempDir::new("autosave-many");
        let mut f = fixture_in(&dir);
        for _ in 0..20 {
            new_sprite(&mut f);
            f.draw_line(2);
        }
        autosave(&mut f.state, false);
        assert_eq!(f.state.recovery.writing.len(), 1);
        settle(&mut f.state, true);
        assert_eq!(copies(&dir).len(), 40);
    }

    #[test]
    fn without_a_recovery_folder_nothing_happens() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        autosave(&mut f.state, true);
        assert_eq!(tick(&mut f.state, 1e9), None);
        clean_exit(&mut f.state);
        check_at_start(&mut f.state);
        assert_eq!(f.state.dialog, None);
        assert!(f.state.log.is_empty());
    }
}
