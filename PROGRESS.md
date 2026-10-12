# ACMS mk3 — Progress

See `PLAN.md` for the full plan. This file tracks what is done, what is next and known bugs.

## Current state

**Phases 0 to 4 are done** (skeleton, drawing, layers, selections and shape tools, pro pixel
features). Next is Phase 5 (not started).

Resolved versions: Rust 1.97.1, egui / eframe 0.36.2 (wgpu backend, eframe's default),
egui_dock 0.21.1, rfd 0.17, zip 9 (deflate only), png 0.18, directories 6, serde 1,
serde_json 1, thiserror 2, anyhow 1, arboard 3.6. Phase 4 added no dependency.

## Phase 4 task list (done)

Core:

- [x] Stroke pipeline stages: pixel-perfect filter, symmetry (horizontal, vertical, both, radial N-way), tiled (wrap-around) plotting
- [x] Custom brushes (an image as the brush; made from the selection)
- [x] Dither ink (Bayer 2×2 / 4×4 / 8×8, checker, custom pattern) and shade ink (one step along a palette ramp)
- [x] Palette: undoable changes, ramps, sort, generate a ramp between two colours, replace a colour across the sprite, reduce the sprite to the palette
- [x] Reference image layer (shown at screen resolution, never composited or exported)

IO:

- [x] Palette import / export: `.gpl`, `.hex`, `.pal`; palette from an image
- [x] `.acms` format version 3 (ramps, symmetry, reference layers) with a migration from version 2; a Phase 3 file kept in `tests/samples/`
- [x] Golden images: pixel-perfect, each symmetry mode, custom brush, dither ink, shade ink, tiled, and all of symmetry + pixel-perfect + dither in one stroke

App:

- [x] Commands, shortcuts and menus for all of the above
- [x] Tool Options: ink, pattern picker, density, pixel-perfect; symmetry and tiled mode
- [x] Canvas: symmetry axes, 3 × 3 tiled preview, reference layers
- [x] Palette panel: selecting colours, ramps, palette commands
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (355 tests); commits along the way

### How Phase 4 was checked

- **Symmetry + pixel-perfect + dither in one stroke** (the "done when"): one test draws a
  stepped freehand line with the pencil with all three on. It checks that every pixel has its
  mirror images, that the line is unbroken but no pixel is the corner of an L, that each pixel
  has the colour the pattern gives its own position, that the stroke is exactly one undo step,
  and that undo and redo restore the sprite exactly. The same is checked again through the
  commands, and there is a golden image of it.
- **Each stage on its own**: the pixel-perfect filter (stairs become a diagonal, the newest pixel
  is shown until it turns out to be a corner, straight lines are untouched); every symmetry mode
  on even and odd canvases; radial symmetry at 3, 5, 6 and 7 copies leaves no gaps; tiled strokes
  wrap; custom brushes; every dither pattern and density; shade ink forwards, back, at the ends
  of a ramp and once per stroke.
- **Palette**: every change undoes exactly; sorting, ramps, generated ramps, replacing a colour
  on several layers (locked ones are left alone), reducing to the palette.
- **Reference layers**: fitted, never part of the flattened picture or an exported PNG, cannot
  be drawn on or merged, follow canvas resize and sprite scale, survive flatten, undo exactly.
- **Format**: version 3 round-trips; a hand-built version 2 manifest and a real file saved by
  the Phase 3 build both load; damaged version 3 parts are errors, not panics.
- **Palette files**: all three formats round-trip, match what other programs write, read files
  written by other programs (as text in the tests), and report broken lines by number.
- **Golden images** (11 new), looked at enlarged before being committed. The ones from earlier
  phases are unchanged, so nothing that existed draws differently.
- **Headless UI tests**: drawing on the canvas with symmetry, tiled mode and reference layers
  shown; Tool Options drawn for every tool and ink; Ctrl+B.
- **In the real window**: captures of symmetry "both" and radial 6 with their axes, a dithered
  pixel-perfect stroke, a reference picture under the drawing, the tiled preview with a stroke
  wrapping around, the top of Tool Options, the Palette panel with selected
  swatches, and a reference row in the Layers panel.
- **Not checked by me**: drawing with a real mouse; the file dialogs for palettes and reference
  pictures; Ctrl / Shift clicking swatches; the pattern picker and the ramps list being clicked
  (they are below the fold of their panels in the captures); palette files actually exported by
  GIMP, Lospec or Paint Shop Pro.

### Known limits of Phase 4

- **Symmetry is always about the middle of the canvas.** The mirror lines cannot be moved.
- **Radial symmetry with a count other than 2 or 4 has to round.** Lines are joined up so they
  have no gaps, but filled shapes and thick brushes can show single-pixel holes in the copies.
- **Pixel-perfect only applies to freehand lines** (pencil and eraser), and it cuts the corner
  of a sharp turn, as the filter does in other editors.
- **A custom brush or pattern is at most 64 × 64**, lives for the session, and is not saved.
  Picking Round or Square drops the custom brush (Ctrl+B makes it again).
- **Tiled mode wraps what is drawn**, not what is filled or moved: the bucket's area and the
  move tool stop at the canvas edge. The selection outline is only drawn on the middle tile.
- **Reference layers**: PNG only, at most 4096 pixels on a side; placed as large as fits, in the
  middle, and cannot be moved or scaled yet. A reference below every image layer is drawn under
  the sprite, any other one over it — it is never mixed in between two layers.
- **Palette files hold neither transparency nor ramps.** Importing replaces the palette.
- **Reduce to Palette** goes by red, green and blue distance and keeps each pixel's alpha.
- **Shade ink needs an exact match**: a pixel whose colour is in no ramp is left alone.
- At the default window size, Tool Options scrolls to reach the pattern picker and symmetry.

## Phase 3 task list (done)

Core:

- [x] Selection as a mask on the sprite: rectangle, lasso (polygon), magic wand; replace / add / subtract / intersect; invert, select all, deselect; undoable
- [x] Selection is a mask stage of the stroke pipeline, so every tool respects it
- [x] Shape tools: line, rectangle, ellipse (outline and filled)
- [x] Bucket fill (contiguous / global, tolerance), gradient (dithered or smooth)
- [x] Move tool; cut / copy / paste / delete; flip, rotate 90°, nearest-neighbour scale of the selection
- [x] Canvas resize, crop to selection, sprite scale
- [x] Several open documents in the editor

IO:

- [x] Golden images for line, rectangle, ellipse, bucket, gradient, and for drawing inside a selection

App:

- [x] Commands, shortcuts and menus for all of the above; system clipboard as an image
- [x] Canvas: selection outline (marching ants), tool previews, modifier keys for selection modes
- [x] Tool Options per tool; dialogs for canvas size, sprite size, scale selection
- [x] Tabs for several open sprites (moved here from section 5, agreed 2026-10-11)
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (283 tests); commits along the way

### How Phase 3 was checked

- **Every painting tool respects the selection and is one undo step**: one test runs pencil,
  eraser, line, rectangle, ellipse, bucket and gradient across a selection, checks that no pixel
  outside it changed, that exactly one history entry appeared, and that undo and redo restore
  the sprite exactly. The move tool has its own tests (with and without a selection, overlapping
  its own old position, cancelled).
- **Edits**: copy, cut, paste, delete, flip, rotate and scale each undo exactly; canvas resize,
  crop and sprite scale too, including getting cut-off pixels and the selection back.
- **Golden images** (`shapes`, `bucket`, `gradient`, `selection`), looked at enlarged.
- **Commands**: clipboard, select all / none / invert, transforms, canvas size with anchors,
  crop, sprite size, tool settings, several sprites each keeping file, view and unsaved state,
  reopening an already open file.
- **Headless UI tests**: dragging with the rectangle-select tool selects and the pencil then
  stays inside; Ctrl+D / Ctrl+A; clipboard shortcuts in the form the window really sends them;
  switching and closing sprite tabs.
- **In the real window**: a capture with two tabs, a dithered gradient, an ellipse, a lasso
  selection with its outline, the full tool list and the bucket's options.
- **Not checked by me**: the system clipboard with another program, the three size dialogs and
  the new menus being clicked, selection modifier keys with a real keyboard.

### Known limits of Phase 3

- **Ctrl+V depends on how the keys are released.** The windowing layer swallows the Ctrl+V key
  press when the clipboard holds no text (as after copying pixels), so paste is triggered by the
  *release* of V while Ctrl is still down. Releasing Ctrl first does nothing. Edit → Paste and
  the command palette always work. A proper fix needs a change in egui-winit or a clipboard crate
  that can put text and image on the clipboard together.
- The selection is not saved in the `.acms` file.
- Moving pixels off the canvas loses them (undo brings them back).
- Paste lands where the pixels were copied from, or centred if that does not fit; it is not
  attached to the mouse.
- The gradient fills the selection, or the whole canvas without one.
- Magic wand and bucket look at the selected layer only, not at all layers together.
- Sprite tabs are a strip above the canvas, not dockable tabs; sprites cannot be shown side by side.

## Phase 2 task list (done)
Core:

- [x] Layer tree: image layers (opacity, blend mode, visible, locked, alpha lock) and groups (children, visible, collapsed)
- [x] Undoable layer operations in transactions: add, delete, duplicate, rename, move / reorder, merge down, flatten, and every property change
- [x] Slider drags (opacity) collapse into one undo step
- [x] Blend modes: normal, multiply, screen, overlay, add; layer opacity; hidden layers and groups
- [x] Compositor keeps the flattened frame and only recomposites the damaged rectangle
- [x] Tools respect lock, alpha lock and hidden layers

IO:

- [x] `.acms` format version 2 (layer tree and properties) with a migration from version 1; a Phase 1 file kept in `tests/samples/`
- [x] Golden images for compositing (each blend mode, opacity, groups, hidden layers)

App:

- [x] Layer commands and shortcuts
- [x] Layers panel: select, visibility / lock / alpha-lock toggles, rename, drag to reorder, groups, opacity, blend mode
- [x] Menu bar and status bar (moved here from the open questions, agreed 2026-10-11)
- [x] `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` pass (234 tests); commits along the way

### How Phase 2 was checked

- **Every layer operation undoes exactly**: a test helper runs each one as a transaction and
  compares the whole sprite before, after undo and after redo. Covered: add layer, add group,
  delete (also a whole group), duplicate (also a whole group), move within a list / into a group /
  out of it, rename, visibility, opacity, blend mode, lock, alpha lock, merge down, flatten.
- **Compositing matches golden images**: one per blend mode (`blend_*.png`) and one for opacity,
  groups and hidden layers (`layers_opacity_groups.png`), all looked at enlarged before being
  committed. Blend maths is also checked against hand-computed values and identities.
  Merging down and flattening are checked to leave the picture pixel-identical.
- **The cached compositor equals a full recomposition** after every kind of change, undo and redo.
- **Format**: layered sprites round-trip; a hand-built version 1 manifest and a real file saved by
  the Phase 1 build both load.
- **Headless UI tests**: clicking a row selects it, the eye hides a layer (and the pencil then does
  nothing on it), dragging a row reorders or moves it into a group, F2 renames in place, a folded
  group hides its rows, the status bar reports the pixel and colour under the pointer.
- **In the real window**: launched with a layered sprite and inspected in captures, dark and light:
  multiply stripe, a 63 % layer inside a group, a hidden layer, the Layers panel, menu bar and
  status bar.
- **Performance** (target: 60 fps while drawing on 512 × 512 with 20 layers): with 20 full-canvas,
  translucent layers in mixed blend modes, one stroke update with a 16 px brush takes about
  0.5 ms on average and 3.3 ms at worst. Recomposing the whole canvas (opening, or changing a
  full-canvas layer's opacity) takes about 73 ms in that worst case.
- **Not checked by me**: the menus being opened and clicked, the opacity slider and blend-mode
  dropdown being used by hand, and drag-reordering with a real mouse.

### Known limits of Phase 2

- Groups have visibility only, as in `PLAN.md` 4.1: no opacity or blend mode of their own.
- Dragging the opacity of a full-canvas layer on a large, many-layered sprite recomposes the
  whole canvas each step (about 73 ms for 512 × 512 with 20 layers), so it is not smooth there.
- At the default 1280 × 800 window the Layers panel shows about four rows before it scrolls.
- Layer rows show icons drawn in code (eye, padlock, checkerboard); there is no icon set yet.
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
- **Layers are a nested tree** (`Sprite::layers()` are the top-level layers; a group owns its
  children). A position is a `Slot { parent, index }`, index counted from the bottom.
- **Layer ids are never reused**, not even after undoing the layer's creation, so an id in a
  script or a keymap always means the same layer.
- **Folding a group is view state**: it is saved with the file but is not an undo step and does
  not mark the sprite as changed. Selecting a layer is not an undo step either. Everything else
  about layers is.
- **Visibility, lock and alpha lock are undoable**, like every other property change.
- **Blending follows the usual compositing rule**: a blend mode only shows where there is
  something below. On transparent pixels every mode looks like Normal.
- **Flatten keeps what you see**: hidden layers are dropped. **Merge down** applies the upper
  layer's opacity and blend mode and keeps the lower layer's own properties.
- **Tools do not draw on a locked or hidden layer, or with a group selected.** The cursor turns
  into "not allowed" and the status bar says why. A locked layer also refuses pixel changes from
  any other source; merging onto it is refused.
- **Alpha lock** recolours existing pixels and keeps their alpha; the eraser does nothing.
- **`.acms` format version 2.** Version 1 files are upgraded when loaded; saving writes version 2.
- **The selection lives on the sprite and is undoable**, but is not written to the file.
- **Selecting with a plain click deselects.** Shift adds, Alt subtracts, Shift+Alt intersects.
- **Opening or creating a sprite never replaces another**: it gets a tab. Only Close and Quit ask
  about unsaved changes.
- **System clipboard crate**: `arboard` (already a dependency of egui-winit), added to the tech
  table. Copy puts the image on both the app's own clipboard and the system one.
- **Dev builds optimise `acms-core`** (`[profile.dev.package.acms-core]`), so `cargo run` stays
  responsive on large sprites.
- **Symmetry is saved with the sprite but is not an undo step** and does not mark the sprite as
  changed, like folding a group: it is how the sprite is worked on, not what is in it.
  Pixel-perfect, tiled mode, the ink and the pattern belong to the editor, not to a sprite.
- **Which strokes are mirrored**: freehand lines and shapes. Bucket, gradient and move are not.
- **Tiled mode is one switch** for both wrap-around strokes and the 3 × 3 preview.
- **A custom brush stamps its own colours** with the Simple ink; the eraser, the dither ink and
  the shade ink only use its shape.
- **Dither**: the primary button puts the primary colour where the pattern says "first", the
  secondary button swaps the two. A transparent colour in the pattern erases.
- **Shade**: the primary button moves a pixel one step towards the end of the first ramp that
  contains its colour, the secondary button one step back. Ramps hold colours, not palette
  positions, so sorting the palette does not disturb them.
- **A reference picture is not pixels of the sprite.** It keeps its own resolution, is stretched
  over a rectangle of the canvas and is drawn by the view, so a photo stays readable on a 32 × 32
  sprite. In the model it is a layer kind holding the picture and that rectangle (`PLAN.md` 4.1
  lists only `visible` and `opacity`). A new one goes at the bottom of the layer list.
- **Palette changes are undo steps**; replacing a colour and reducing to the palette skip locked
  layers rather than fail.
- **`.acms` format version 3.** Version 1 and 2 files are upgraded when loaded.
- **Added without being in the plan's list** (2026-10-12, to be confirmed): "Add Primary Colour
  to Palette" and "Remove Selected Colours" — without them ramps could only be made from
  imported or generated colours.

## Open questions

The four section 5 items without a phase were placed on 2026-10-11 and written into
`PLAN.md`: menu bar and status bar → Phase 2, tabs for several sprites → Phase 3, Preview
panel → Phase 5.

Open since Phase 4:

- **Palette grid with drag-reorder** (section 5) is in no phase. The palette can be sorted, and
  colours added and removed, but not dragged into an order.
- **Should `PLAN.md` 4.1 describe the reference layer as built** (picture and rectangle in the
  layer, drawn by the view)? Not changed yet; see "Decisions".
- **Keep "Add / Remove colour"?** They are not in the Phase 4 list of palette tools.

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
| L / U / Shift+U | Line / Rectangle / Ellipse |
| G / Shift+G | Bucket fill / Gradient |
| V / M / Q / W | Move / Rectangle select / Lasso / Magic wand |
| Ctrl+X / C / V, Delete | Cut / Copy / Paste, delete the selected pixels |
| Ctrl+A / Ctrl+D / Ctrl+Shift+I | Select all / Deselect / Invert selection |
| Ctrl+W, Ctrl+Tab / Ctrl+Shift+Tab | Close sprite, next / previous sprite |
| Ctrl+B | Brush from the selection |
| Ctrl+Shift+N | New layer |
| F2 | Rename the selected layer |
| X | Swap primary and secondary colour |
| Ctrl+Plus / Ctrl+Minus / Ctrl+0 | Zoom in / Zoom out / Zoom to fit |
| Ctrl+G | Toggle pixel grid |
| Ctrl+Shift+P or Ctrl+K | Command palette |
| Ctrl+Shift+T / Ctrl+Shift+R | Toggle theme / Reset panel layout |
| Ctrl+Q | Quit |

Mouse: wheel zooms at the pointer, middle-drag or Space + drag pans, left button draws with the
primary colour, right button with the secondary colour, Escape abandons the stroke in progress.

Layers panel: click a name to select, double-click (or F2) to rename, drag a row to reorder it
or drop it on a group to move it inside. The icons on each row toggle visibility, lock and alpha
lock. The other layer commands (merge down, flatten, move up / down, …) are in the Layer menu
and the command palette.

Palette panel: click a swatch for the primary colour, right click for the secondary one. Ctrl
click adds a swatch to the selection, Shift click selects a run; "Ramp" makes a ramp from the
selected swatches in the order they were picked. Everything else about the palette is in the
Palette menu. Inks, patterns, pixel-perfect, symmetry and tiled mode are in Tool Options and,
like everything, in the command palette (try "ink", "symmetry", "tiled").

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
- egui_dock keeps every panel at least `style.separator.extra` points large (default 175) and
  applies that to the default layout too; `theme::dock_style` lowers it to 64.
- A panel whose content changes height with the selection makes its rows jump under the pointer
  (this broke drag-reordering once). Keep such controls in place and disable them instead.
- Headless UI tests click at positions recorded by the panels; a widget that is laid out outside
  the visible part of its panel cannot be clicked, which is how the Layers panel being too
  cramped was found.
- Phase 3 will need: selections as a mask stage in `stroke.rs` (next to `lock_alpha`), more ops
  in `document.rs` for canvas resize, and `AppState` holding several documents for sprite tabs.
- A stroke's settings are fixed when it begins (`StrokeSettings`); tools ask for a
  `StrokeKind` (freehand, shape, plain) and the pipeline decides what applies. A new ink is a
  variant of `Ink` plus a case in `tool::ink_for`.
- The preview buffer can take pixels back (`StrokeBuffer::take_back`), which is how the
  pixel-perfect filter removes a corner it has already shown. Its `bounds` may therefore be
  larger than what is touched; `get` is what counts.
- Phase 5 will meet reference layers again: they have no cels, so they are the same in every
  frame, and the compositor already skips them.
- PowerShell 5.1 reads scripts without a BOM as ANSI: keep non-ASCII text out of helper scripts
  (it silently broke a search-and-replace on "…").
- If full-canvas recomposition becomes a problem, the place to optimise is the per-pixel loop in
  `compositor::compose_rect` (row slices instead of `get` / `set`, a fast path for opaque Normal).

## Known bugs

None known.

## Ideas

(Things that are not in the plan go here instead of into the code.)

- Commands to show a single closed panel again (today only "Reset Panel Layout" brings panels back).
- A command that opens the keymap file (creating it if missing) in the system editor.
- Open a file given on the command line (needed for double-click in the file manager).
- Shortcuts to grow / shrink the brush (`[` and `]`).
- Opacity and blend mode for groups.
- A "solo" click on the eye (hide every other layer).
- Show the file name and unsaved mark in the window title, not only on the canvas tab.
- Movable symmetry axes (drag the mirror lines, or centre them on the selection).
- Move and scale a reference picture with the move tool; reference pictures from JPEG files.
- Tiled mode for the bucket and the move tool (fill and move across the edge).
- Keep custom brushes and patterns in a list, and save them.
- A two-column or icon Tools panel, so Tool Options gets more room.
