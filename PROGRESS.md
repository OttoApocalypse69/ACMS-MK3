# ACMS mk3 — Progress

See `PLAN.md` for the full plan. This file tracks what is done, what is next and known bugs.

## Current state

**Phase 0 — Skeleton: done. Phase 1 — Draw on a canvas: done. Phase 2 — Layers: in progress.**

Resolved versions: Rust 1.97.1, egui / eframe 0.36.2 (wgpu backend, eframe's default),
egui_dock 0.21.1, rfd 0.17, zip 9 (deflate only), png 0.18, directories 6, serde 1,
serde_json 1, thiserror 2, anyhow 1.

## Phase 2 task list

Core:

- [ ] Layer tree: image layers (opacity, blend mode, visible, locked, alpha lock) and groups (children, visible, collapsed)
- [ ] Undoable layer operations in transactions: add, delete, duplicate, rename, move / reorder, merge down, flatten, and every property change
- [ ] Slider drags (opacity) collapse into one undo step
- [ ] Blend modes: normal, multiply, screen, overlay, add; layer opacity; hidden layers and groups
- [ ] Compositor keeps the flattened frame and only recomposites the damaged rectangle
- [ ] Tools respect lock, alpha lock and hidden layers

IO:

- [ ] `.acms` format version 2 (layer tree and properties) with a migration from version 1; a Phase 1 file kept in `tests/samples/`
- [ ] Golden images for compositing (each blend mode, opacity, groups, hidden layers)

App:

- [ ] Layer commands and shortcuts
- [ ] Layers panel: select, visibility / lock toggles, rename, drag to reorder, groups, opacity, blend mode, alpha lock
- [ ] Menu bar and status bar (moved here from the open questions, agreed 2026-10-11)
- [ ] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass; commits along the way

## Phase 1 task list (done)

Core (`acms-core`, no egui, no file IO):

- [x] `Rgba8`, `Point` / `Rect`, `Image` (RGBA8, straight alpha)
- [x] Document model: `Sprite` (size, palette, layers, frames, cels by `(LayerId, frame)`), stable `LayerId`, cels cropped to their content with an origin
- [x] Transactions: every pixel change goes through one, stores only the changed region (before / after)
- [x] History: list with a cursor, undo / redo / jump, memory limit, revision number for "unsaved changes"
- [x] Stroke pipeline: interpolate (Bresenham) → brush stamp (square / round, size N) → ink (simple, erase) → canvas mask → preview buffer → commit as one transaction
- [x] Compositor: flatten a frame (or a rectangle of it), with the stroke preview on top
- [x] `Tool` trait + tools: pencil, eraser, eyedropper
- [x] `Editor`: document, primary / secondary colour, brush, tool registry and active tool
- [x] Typed command parameters (deferred from Phase 0)

IO (`acms-io`):

- [x] `.acms` save / load (zip: `manifest.json` with `format_version`, `cels/<id>.png`, `thumb.png`)
- [x] PNG export with integer scale
- [x] Round-trip tests; golden image tests for pencil, eraser and eyedropper in `tests/golden/`

App (`acms-app`):

- [x] Commands: new, open, save, save as, export PNG, undo, redo, history jump, tool selection, colours, brush, zoom, pan, grid
- [x] Keymap: default shortcuts for the above; optional `args` per binding
- [x] Viewport: sprite texture (nearest), checkerboard, pixel grid, zoom at cursor in integer steps, pan with middle mouse or space + drag
- [x] Pointer input → active tool, no gaps on fast strokes
- [x] Panels: Tools, Tool Options (brush), Colour (primary / secondary, picker, hex), Palette (basic grid), History
- [x] Dialogs: new sprite, export PNG, native file dialogs (`rfd`), unsaved-changes confirmation
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (160 tests); commits along the way

### How Phase 1 was checked

- **Core unit tests** (70): colour maths, images, the sprite model, transactions
  (do → undo → redo gives identical data, failed transactions roll back, only the changed area
  is stored, history limit), every stage of the stroke pipeline, compositor, tools, editor,
  command parameters.
- **IO tests** (15): `.acms` save → load → compare, damaged and too-new files, atomic saves,
  PNG encode / decode / export. A saved file was also opened with an independent zip reader.
- **Golden images** (5, `tests/golden/`): scripted strokes through pencil, eraser and eyedropper.
  The images were looked at enlarged before being committed. Regenerate with
  `ACMS_UPDATE_GOLDEN=1 cargo test -p acms-io --test golden`.
- **Command tests**: new / open / save / save as / export through real files, failures,
  unsaved-changes prompts, undo / redo / jump, tools, colours, brush, zoom, pan.
- **Headless UI tests** (`app.rs`): real pointer, wheel and key events through full egui frames —
  dragging draws one undoable stroke, a fast drag leaves no gaps, right button uses the secondary
  colour, tool shortcuts, Escape cancels a stroke, the wheel zooms around the pointer, middle-drag
  pans, a dialog blocks the canvas and shortcuts, closing the window asks about unsaved changes.
- **In the real window**: launched with a drawn sprite and inspected in captures — canvas,
  checkerboard, pixel grid, translucent pixels, all panels, History after undo, the export dialog,
  the command palette, dark and light theme.
- **Start-up time** (target: under 1 second): the release build shows its window in about
  0.35 s; the very first start after building took 1.2 s.
- **Not checked by me**: drawing with a real mouse in the real window, the native file dialogs,
  and frame rate while drawing. These need a person at the machine.

### Known limits of Phase 1

- One sprite is open at a time (see "Open questions").
- The colour picker's R / G / B readout can be off by one for translucent colours (egui converts
  through HSV). The colour itself stays exact until it is edited with the picker.
- The eyedropper ignores fully transparent pixels instead of picking "nothing".
- `thumb.png` in an `.acms` is the flattened first frame at 1×, not a scaled thumbnail.

## Decisions

- **Command context** (agreed 2026-10-11, in `PLAN.md` 4.2). `CommandRegistry<C>` stays in
  `acms-core`, generic over its context. The app's context is `AppState`; its document-related
  part is `acms_core::editor::Editor` (document, colours, brush, tools).
- **Command parameters.** A command declares named parameters with a type (bool, whole number,
  number, text, colour) and whether they are required. The registry checks and converts arguments
  before the command runs. A command that needs something it was not given asks for it: a dialog
  (`file.new`, `file.export_png`) or a native file dialog (`file.open`, `file.save_as`), which then
  runs the same command again with the arguments.
- **Config folder crate** (agreed 2026-10-11): `directories`.
- **Keymap file holds overrides, not the whole keymap**, so shortcuts added in later versions
  reach users who already have a file.
- **Cels are always cropped to their content**: no fully transparent border, an empty cel is
  simply absent, and every fully transparent pixel is stored as `(0, 0, 0, 0)`. This is what
  makes undo return byte-identical data.
- **Tools are not commands.** Pointer input goes canvas → `Editor` → active tool → transaction,
  as in `PLAN.md` 4.4 / 4.5. Everything else the UI does is a command.
- **Sprite size limit: 4096 × 4096** (`MAX_SPRITE_SIZE`). The canvas is one texture and this
  size is safe on every GPU. To be revisited if larger canvases are wanted.
- **Unsaved changes are never dropped silently**: New, Open, Quit and closing the window ask
  first. This is not spelled out in the plan; it was added because Phase 1 introduces saving.

## Open questions

None. The four section 5 items without a phase were placed on 2026-10-11 and written into
`PLAN.md`: menu bar and status bar → Phase 2, tabs for several sprites → Phase 3, Preview
panel → Phase 5.

## The keymap file

Read once at startup from `%APPDATA%\acms-mk3\config\keymap.json` (Windows) or
`~/.config/acms-mk3/keymap.json` (Linux). The app never writes it; that comes with the keymap
editor in Phase 8. The Log panel says at startup which file was used and what was skipped.

```json
{
  "bindings": [
    { "keys": "Ctrl+J", "command": "view.toggle_grid" },
    { "keys": "Ctrl+1", "command": "brush.set_size", "args": { "size": 1 } },
    { "keys": "Ctrl+G", "command": null }
  ]
}
```

- Each entry binds a chord to a command id, replacing what that chord did. `null` unbinds it.
- `args` are passed to the command. Numbers, text and true / false are allowed; a colour is
  written as text (`"#FF8800"`).
- Only changes go in the file; everything not mentioned keeps its default.
- A file that is not valid is ignored as a whole (defaults are used). An entry with an
  unparseable chord or unusable arguments is skipped. A binding to an unknown command id is kept
  and reported.

### Default shortcuts

| Keys | Command |
|---|---|
| Ctrl+N / Ctrl+O | New sprite / Open |
| Ctrl+S / Ctrl+Shift+S | Save / Save As |
| Ctrl+E | Export PNG |
| Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y | Undo / Redo |
| B / E / I | Pencil / Eraser / Eyedropper |
| X | Swap primary and secondary colour |
| Ctrl+Plus / Ctrl+Minus / Ctrl+0 | Zoom in / Zoom out / Zoom to fit |
| Ctrl+G | Toggle pixel grid |
| Ctrl+Shift+P or Ctrl+K | Command palette |
| Ctrl+Shift+T / Ctrl+Shift+R | Toggle theme / Reset panel layout |
| Ctrl+Q | Quit |

Mouse: wheel zooms at the pointer, middle-drag or Space + drag pans, left button draws with the
primary colour, right button with the secondary colour, Escape abandons the stroke in progress.

## Phase 0 task list (done)

- [x] `git init`, Rust `.gitignore`
- [x] Cargo workspace with `acms-core`, `acms-io`, `acms-script`, `acms-app` (dependency direction app → script → io → core)
- [x] Dependencies added with `cargo add`; egui / eframe / egui_dock sources read for the resolved versions
- [x] `acms-core`: command registry, no egui, unit tests
- [x] `acms-app`: eframe window (wgpu backend), `theme.rs` tokens (dark + light), docking with a default layout
- [x] `acms-app`: keymap, command palette with fuzzy search and shortcuts
- [x] Keymap file: user bindings loaded from the config folder on top of the defaults

## Deferred from the plan's description

Described in `PLAN.md` but not part of the "Done when" of a finished phase:

- **Custom fonts and icon set** (5). Text sizes, spacing and colours are set explicitly, but the
  typeface is still egui's bundled one and tools are text buttons; `assets/` does not exist yet.
- **`log` crate wiring** (2). The Log panel shows lines written by commands; the `log` facade is
  not hooked up to it yet.
- **Saved layouts** (5). The layout resets on restart.
- **Custom brush images** (4.4). Square and round only; custom brushes are Phase 4.
- `scripts/` and `assets/` folders are created when they first have content.

## Notes for later phases

- egui 0.36: `eframe::App::ui(&mut self, ui, frame)` receives a `Ui`, not a `Context`;
  panels use `.show(ui, ..)`; styles are set per theme with `Context::set_style_of`.
- egui matches shortcuts ignoring extra Shift/Alt, so the keymap checks chords with more
  modifiers first. Its own Ctrl+Plus / Minus / 0 interface zoom is switched off in `theme::install`
  so those keys can zoom the canvas.
- egui_dock 0.21: the fraction passed to `split_left` / `split_above` is the share of the *new*
  (left / top) node, despite what the method docs say. `split_right` / `split_below` give the
  share of the existing node.
- While the dock is shown it is taken out of `AppState`, so panels can be handed the whole state.
  A command that replaces the layout bumps `layout_generation` so the old one is not put back.
- eframe does not run the UI while its window is covered, so the window cannot be driven with
  posted messages from a script; to look at a specific state, start the app in that state.
- Right after a new egui release, `cargo add` can fail to resolve sibling crates until the
  registry index catches up; retrying fixed it.
- Phase 2 will need: `Layer` to become the enum from `PLAN.md` 4.1, new undo operations next to
  `Op::Pixels` in `document.rs`, and a compositor cache if 20 layers are too slow to flatten per
  stroke update.

## Known bugs

None known.

## Ideas

(Things that are not in the plan go here instead of into the code.)

- Commands to show a single closed panel again (today only "Reset Panel Layout" brings panels back).
- A command that opens the keymap file (creating it if missing) in the system editor.
- Open a file given on the command line (needed for double-click in the file manager).
- Shortcuts to grow / shrink the brush (`[` and `]`).
- Show the file name and unsaved mark in the window title, not only on the canvas tab.
