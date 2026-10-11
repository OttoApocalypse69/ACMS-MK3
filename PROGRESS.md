# ACMS mk3 — Progress

See `PLAN.md` for the full plan. This file tracks what is done, what is next and known bugs.

## Current state

**Phase 0 — Skeleton: done.** Next is Phase 1 (not started).

Resolved versions: Rust 1.97.1, egui / eframe 0.36.2 (wgpu backend, eframe's default),
egui_dock 0.21.1, thiserror 2, anyhow 1.

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
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (33 tests)
- [x] Commit

### How Phase 0 was checked

- Unit tests for the registry, fuzzy matcher, chord parser, keymap ordering, default layout and
  built-in commands.
- Headless UI tests in `app.rs` feed real key and text events through full egui frames:
  shortcuts run commands, both palette shortcuts open it, typing filters, arrows move the
  selection, Enter runs, Escape closes, disabled commands do not run, Quit sends the close command.
- The window was launched and its default layout inspected in a capture. Dragging panels, the
  look of the palette and the light theme were **not** checked by hand yet.

## Deferred from the plan's description (decide in the phase noted)

These are described in `PLAN.md` but are not part of Phase 0's "Done when", so they were left out
rather than half-built:

- **Typed command parameters** (4.2). Commands currently take no parameters. Needed by Phase 1's
  first parameterised command.
- **Keymap stored in a user config file** (4.2). Bindings are a table in `keymap.rs`; chords are
  already parsed from text (`"Ctrl+Shift+P"`) so a file can use the same format. Needs a crate
  for the config directory, which is not in the plan's tech table — to be agreed first.
- **Custom fonts and icon set** (5). Text sizes, spacing and colours are set explicitly, but the
  typeface is still egui's bundled one; `assets/` does not exist yet.
- **`log` crate wiring** (2). The Log panel shows lines written by commands; the `log` facade is
  not hooked up to it yet.
- **Saved layouts** (5). The layout resets on restart.
- `scripts/`, `assets/` and `tests/` folders are created when they first have content.

## Open questions for the plan

- `CommandRegistry<C>` is generic over the context a command acts on, so `acms-core` stays free
  of app types. In Phase 0 the context is the app's `AppState`. When scripts arrive (Phase 7),
  `acms-script` sits *below* `acms-app` and cannot name `AppState`, so by then the context commands
  receive has to be a type both can see. Worth settling when the document model lands in Phase 1.

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
