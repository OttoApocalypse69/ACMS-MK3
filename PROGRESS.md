# ACMS mk3 — Progress

See `PLAN.md` for the full plan. This file tracks what is done, what is next and known bugs.

## Current state

**Phase 0 — Skeleton: done.** Next is Phase 1 (not started).

Resolved versions: Rust 1.97.1, egui / eframe 0.36.2 (wgpu backend, eframe's default),
egui_dock 0.21.1, thiserror 2, anyhow 1, directories 6, serde 1, serde_json 1.

## Phase 0 task list

- [x] `git init`, Rust `.gitignore`
- [x] Cargo workspace with `acms-core`, `acms-io`, `acms-script`, `acms-app` (dependency direction app → script → io → core)
- [x] Dependencies added with `cargo add`; egui / eframe / egui_dock sources read for the resolved versions
- [x] `acms-core`: command registry (id, display name, `is_enabled`, `run`), no egui, unit tests
- [x] `acms-app`: eframe window (wgpu backend)
- [x] `acms-app`: `theme.rs` with named tokens, dark + light, applied to egui and egui_dock
- [x] `acms-app`: docking with placeholder panels and a default layout
- [x] `acms-app`: keymap (key chord → command id) with default bindings
- [x] `acms-app`: command palette with fuzzy search, shortcut shown next to each command
- [x] A few dummy commands registered and bound
- [x] Keymap file: user bindings loaded from the config folder on top of the defaults
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (42 tests)
- [x] Commit

### The keymap file

Read once at startup from `%APPDATA%\acms-mk3\config\keymap.json` (Windows) or
`~/.config/acms-mk3/keymap.json` (Linux). The app never writes it; that comes with the keymap
editor in Phase 8. The Log panel says at startup which file was used and what was skipped.

```json
{
  "bindings": [
    { "keys": "Ctrl+J", "command": "debug.say_hello" },
    { "keys": "Ctrl+H", "command": null }
  ]
}
```

- Each entry binds a chord to a command id, replacing what that chord did. `null` unbinds it.
- Only changes go in the file; everything not mentioned keeps its default.
- A file that is not valid is ignored as a whole (defaults are used). An entry with an
  unparseable chord is skipped. A binding to an unknown command id is kept and reported.

### How Phase 0 was checked

- Unit tests for the registry, fuzzy matcher, chord parser, keymap ordering, default layout and
  built-in commands.
- Headless UI tests in `app.rs` feed real key and text events through full egui frames:
  shortcuts run commands, both palette shortcuts open it, typing filters, arrows move the
  selection, Enter runs, Escape closes, disabled commands do not run, Quit sends the close command,
  a keymap file rebinds and unbinds keys.
- The window was launched and its default layout inspected in a capture. Dragging panels, the
  look of the palette and the light theme were **not** checked by hand yet.

## Deferred from the plan's description (decide in the phase noted)

These are described in `PLAN.md` but are not part of Phase 0's "Done when", so they were left out
rather than half-built:

- **Typed command parameters** (4.2). Commands currently take no parameters. Needed by Phase 1's
  first parameterised command.
- **Parameters in keymap bindings** (4.2), which depend on the item above.
- **Custom fonts and icon set** (5). Text sizes, spacing and colours are set explicitly, but the
  typeface is still egui's bundled one; `assets/` does not exist yet.
- **`log` crate wiring** (2). The Log panel shows lines written by commands; the `log` facade is
  not hooked up to it yet.
- **Saved layouts** (5). The layout resets on restart.
- `scripts/`, `assets/` and `tests/` folders are created when they first have content.

## Decisions

- **Command context** (agreed 2026-10-11, written into `PLAN.md` 4.2). `CommandRegistry<C>` stays
  in `acms-core`, generic over its context. The app owns the concrete context (`AppState`).
  From Phase 1 the document-related part of it is a type defined in `acms-core`, held as a field
  of `AppState`; script commands (Phase 7) are written against that core type and wrapped by the
  app, so `acms-script` never needs to name `AppState`. No code change was needed in Phase 0.
- **Config folder crate** (agreed 2026-10-11, added to the tech table): `directories`.
- **Keymap file holds overrides, not the whole keymap**, so shortcuts added in later versions
  reach users who already have a file.

## Notes for later phases

- egui 0.36: `eframe::App::ui(&mut self, ui, frame)` receives a `Ui`, not a `Context`;
  panels use `.show(ui, ..)`; styles are set per theme with `Context::set_style_of`.
- egui matches shortcuts ignoring extra Shift/Alt, so the keymap checks chords with more
  modifiers first.
- egui_dock 0.21: the fraction passed to `split_left` / `split_above` is the share of the *new*
  (left / top) node, despite what the method docs say. `split_right` / `split_below` give the
  share of the existing node.
- Right after a new egui release, `cargo add` can fail to resolve sibling crates until the
  registry index catches up; retrying fixed it.

## Known bugs

None known.

## Ideas

(Things that are not in the plan go here instead of into the code.)

- Commands to show a single closed panel again (today only "Reset Panel Layout" brings panels back).
- A command that opens the keymap file (creating it if missing) in the system editor.
