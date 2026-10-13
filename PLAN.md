# ACMS mk3 — Build Plan

A pixel art editor in Rust. Successor to ACMS-mk2 (C# / WPF / SkiaSharp, pencil + eraser + zoom/pan).
This file is the source of truth for the project. Read it fully before writing code.

---

## 1. What we are building

A desktop pixel art editor that is fast, clean and modern-looking, with four headline areas:

1. **Pro drawing tools** — layers, selections, symmetry, pixel-perfect strokes, dithering and shading inks, strong palette tools.
2. **Animation** — frames, cels, timeline, onion skin, tags, playback, GIF and spritesheet export.
3. **Game-dev features** — tilemap layers, tilesets, seamless-tile preview, slices / 9-slice, engine-friendly export.
4. **Scripting** — Lua scripts can add commands, tools and filters. Everything the UI does goes through the same command system scripts use.

**Feel:** clean modern. Flat, roomy, dockable panels, a command palette, every action has a rebindable shortcut. Not retro-styled, not an Aseprite clone.

### What carries over from mk2

Only the ideas, not the code:

- Core / UI split (mk2: `PixelArtEditor.Core` and `PixelArtEditor.UI`). In mk3 the core has **zero** UI dependencies.
- `ITool` with down / move / up events → the `Tool` trait.
- `ToolService` holding the tool list and the active tool → the tool registry.
- `CanvasRenderer` with zoom/pan → the viewport.
- mk2's own "next steps" (Layer model, Frame model, timeline) are phases 2 and 5 here.

### Not in scope

Vector tools, photo editing, 3D, online collaboration, mobile, AI generation. Do not add them.

---

## 2. Tech stack

| Area | Choice | Notes |
|---|---|---|
| Language | Rust, stable toolchain | |
| Window + UI | `eframe` / `egui`, wgpu backend | |
| Docking | `egui_dock` | Panels can be moved, tabbed, closed |
| Canvas rendering | CPU compositor → one egui texture, nearest-neighbour filtering | Pixel art is small; CPU compositing with dirty rectangles is enough and far simpler than custom shaders |
| Scripting | Lua through `mlua` (vendored Lua) | Same language Aseprite scripts use, easy for users |
| Images | `image`, `png`, `gif` | |
| Project file | `zip` + `serde` / `serde_json` | |
| File dialogs | `rfd` | |
| Config location | `directories` | Finds the per-user config folder (keymap file, later preferences) |
| System clipboard | `arboard` | Copy and paste images to and from other programs |
| Errors | `thiserror` in libraries, `anyhow` in the app | |
| Logging | `log` + a simple in-app log panel | |

**Rule for versions:** do not write crate versions from memory. Use `cargo add` to get the current version, and read the crate's current docs / changelog before using an API (egui's API changes between releases). If a crate in this table is unmaintained or does not fit, propose a replacement and say why before switching.

---

## 3. Workspace layout

```
acms-mk3/
  Cargo.toml              # workspace
  PLAN.md                 # this file
  PROGRESS.md             # what is done, what is next, known bugs (keep updated)
  crates/
    acms-core/            # document model, tools, inks, undo, compositor. NO egui, NO file IO.
    acms-io/              # .acms project format, import/export (png, gif, spritesheet, palettes, tiled)
    acms-script/          # Lua host and API bindings
    acms-app/             # eframe app: panels, viewport, input, theme, keymap, command palette
  scripts/                # bundled example Lua scripts
  assets/                 # fonts, icons, default palettes
  tests/                  # golden images and sample files
```

Dependency direction: `acms-app` → `acms-script` → `acms-io` → `acms-core`. Never the other way.

`acms-core` must compile and pass all its tests without a window. This is what makes tools testable and scripts possible.

---

## 4. Core design

### 4.1 Document model

```
Sprite
  size: (w, h)
  palette: Palette                 # ordered colours, optional names
  layers: Vec<Layer>               # tree; bottom to top
  frames: Vec<Frame>               # duration_ms each
  cels: Map<(LayerId, FrameIdx), Cel>
  tags: Vec<Tag>                   # name, from, to, direction (forward / reverse / ping-pong), colour
  slices: Vec<Slice>               # name, rect, optional 9-slice centre, optional pivot
  tilesets: Vec<Tileset>
  grid: GridSettings
  symmetry: SymmetrySettings

Layer = Image { opacity, blend, visible, locked, alpha_lock }
      | Group { children, visible, collapsed }
      | Tilemap { tileset_id, opacity, visible, locked }
      | Reference { visible, opacity, image, rect }   # never exported; see below

Cel
  origin: (x, y)
  data: Arc<Image>  or  Arc<TileGrid>      # Arc so linked cels share data
```

- Pixels are RGBA8, straight alpha. Colours are picked from the palette but stored as RGBA. (True indexed mode is a later, optional phase.)
- Use stable IDs (`LayerId`, `TilesetId`) rather than vector indices in anything that is stored in undo history or in a script.
- Cels are cropped to their content and have an origin, so a small sprite on a big canvas is cheap.
- A reference layer keeps its picture at the picture's own resolution and a canvas rectangle to stretch it over. It has no cels and is drawn by the view, not by the compositor, so it stays readable on a tiny sprite.

### 4.2 Commands — the centre of the app

Every action is a **command**: a struct with an id (`"layer.new"`, `"edit.flip_horizontal"`), a display name, typed parameters, an `is_enabled(ctx)` check and a `run(ctx)` function.

- Menus, buttons, shortcuts, the command palette and Lua scripts all call commands. No UI code changes the document directly.
- One registry holds all commands. The keymap maps key chords → command id (+ parameters) and is stored in a user config file.
- Commands added by scripts appear in the palette and can be bound to keys like built-in ones.
- The registry lives in `acms-core` and is generic over the context a command acts on, so the core never sees app types. The app owns the concrete context. Everything document-related in that context is a type defined in `acms-core` (from Phase 1); app-only state (dock layout, theme, palette) stays in `acms-app`. Commands registered by scripts are written against the core type and wrapped by the app when it adds them to the registry.
- The keymap file holds only the user's changes; they are applied on top of the built-in defaults, so new default shortcuts reach existing users.

### 4.3 Undo / redo

- All document changes happen inside a **transaction**. A transaction records a list of reversible operations.
- Pixel changes store only the changed region (before and after), not a full image copy. Structural changes (add layer, move frame, etc.) store their inverse.
- One brush stroke = one transaction = one undo step.
- History is a list with a cursor; a history panel shows it and lets the user jump.
- Keep a memory limit for history (configurable).

### 4.4 The stroke pipeline

Tools do not write pixels directly. A stroke flows through stages, so features combine freely:

```
pointer events
  → interpolate           (Bresenham between samples, no gaps)
  → pixel-perfect filter  (removes L-shaped corners, optional)
  → symmetry              (horizontal, vertical, both, radial N-way; optional)
  → brush stamp           (square / round / custom image, size N)
  → ink                   (how a stamped pixel changes the target)
  → mask                  (selection, alpha lock, tile bounds)
  → preview buffer        (shown live; committed as one transaction on release)
```

Inks:
- **Simple** — set colour
- **Erase**
- **Dither** — pattern between primary and secondary colour (Bayer 2×2 / 4×4 / 8×8, checker, custom)
- **Shade** — move a pixel one step lighter/darker along a user-defined palette ramp
- **Lock-alpha** variants of the above
- **Tile** — stamps tiles on a tilemap layer

### 4.5 Tools

`Tool` trait: pointer down / move / up, modifier keys, cancel, cursor/preview hint. Tools are registered in a registry (like mk2's `ToolService`).

Pencil, eraser, line, rectangle, ellipse (outline and filled), bucket fill (contiguous / global, tolerance), gradient with dithering, eyedropper, move, rectangular select, lasso, magic wand, hand (pan), zoom, tile stamp, slice.

### 4.6 Compositor and viewport

- Compositor flattens the visible layers of one frame into an RGBA buffer. It tracks dirty rectangles and only recomposites what changed.
- The app uploads that buffer to a texture and draws it with nearest filtering.
- Overlays are drawn on top with egui's painter: pixel grid, tile grid, selection outline (marching ants), symmetry axes, onion skin (as extra composited frames with tint), slices, tool preview.
- Zoom is integer steps when zoomed in (1×, 2×, 3× …), centred on the cursor. Pan with middle mouse or space + drag.
- Target: 120 fps while drawing on a 512×512 sprite with 20 layers (a frame of the whole app in under 8.3 ms); app start under 1 second.

### 4.7 Project file: `.acms`

A zip archive:

```
manifest.json     # format version, sprite structure, layers, frames, tags, slices, palette, tilesets
cels/<id>.png     # one PNG per unique cel image
tiles/<id>.png    # tileset atlases
thumb.png         # preview
```

- Human-inspectable and easy to debug. Has a `format_version` field from day one; write a migration function whenever it changes.
- Autosave to a recovery folder every N minutes; offer recovery after a crash.

---

## 5. UI

- **Layout:** dockable panels — Tools, Tool Options, Colour, Palette, Layers, Timeline, Tileset, History, Preview, Script Console, Log. A sensible default layout; layouts are saved and restorable.
- **Tabs** for multiple open sprites.
- **Command palette** (`Ctrl+Shift+P`, also `Ctrl+K`): fuzzy search over all commands, shows the shortcut next to each.
- **Theme:** one dark and one light theme, defined in a single `theme.rs` with named tokens (surface, panel, border, text, accent, danger). Generous spacing, rounded corners, one accent colour, consistent icon set. No default-egui look: set fonts, spacing and visuals explicitly.
- **Preview panel:** the sprite at 1× (or chosen scale), playing the animation, with an optional 3×3 tiled mode.
- **Status bar:** cursor position, colour under cursor, selection size, zoom, current frame.
- **Colour UI:** primary/secondary colour, HSV square + hex field, palette grid with drag-reorder.
- Every command reachable by keyboard. Shortcuts shown in tooltips.

---

## 6. Phases

Work through the phases in order. The app must build and run at the end of every phase. Each phase ends with: `cargo fmt`, `cargo clippy` with no warnings, `cargo test` green, `PROGRESS.md` updated, a commit.

### Phase 0 — Skeleton
- Workspace with the four crates, CI-ready (`fmt`, `clippy`, `test`).
- eframe window, theme tokens, docking with empty placeholder panels.
- Command registry, keymap, command palette (with a few dummy commands).

**Done when:** the app opens, panels can be rearranged, the palette lists and runs commands, a shortcut triggers a command.

### Phase 1 — Draw on a canvas (mk2 parity, done properly)
- Sprite with one image layer and one frame. New-sprite dialog.
- Viewport: zoom at cursor, pan, pixel grid, checkerboard background.
- Stroke pipeline with interpolate → brush → simple/erase ink → preview → commit.
- Pencil, eraser, eyedropper. Primary/secondary colour, basic palette panel.
- Transactions and undo/redo.
- Save/load `.acms`, export PNG (with integer scale option).

**Done when:** you can draw, undo, save, reopen and export a PNG. Fast strokes leave no gaps.

### Phase 2 — Layers
- Image layers and groups: add, delete, duplicate, rename, reorder (drag), merge down, flatten.
- Opacity, visibility, lock, alpha lock. Blend modes: normal, multiply, screen, overlay, add.
- Compositor with dirty rectangles.
- Menu bar and status bar (described in section 5).

**Done when:** layer operations all undo correctly and compositing matches golden images.

### Phase 3 — Selections and shape tools
- Selection as a mask: rectangle, lasso, magic wand; add / subtract / intersect; invert, select all, deselect.
- Move / cut / copy / paste (also to and from the system clipboard as an image), flip, rotate 90°, nearest-neighbour scale of the selection.
- Line, rectangle, ellipse, bucket fill, gradient.
- Canvas resize, crop to selection, sprite scale.
- Tabs for several open sprites (described in section 5).

**Done when:** every tool respects the selection mask and is one undo step.

### Phase 4 — Pro pixel features
- Pixel-perfect filter, symmetry modes, custom brushes (make brush from selection).
- Dither ink and shade ink; pattern picker.
- Palette tools: import/export (`.gpl`, `.hex`, `.pal`, from image), add and remove colours, drag to reorder, sort, ramps, generate ramp between two colours, replace colour across sprite, reduce image to palette.
- Tiled drawing mode (wrap-around strokes) with seamless preview.
- Reference image layer.

**Done when:** symmetry + pixel-perfect + dither work together in one stroke.

### Phase 5 — Animation
- Frames, cels, linked cels, per-frame duration.
- Timeline panel: layers × frames grid, drag to move/copy cels and frames, multi-select.
- Tags with playback direction. Play / pause / step, loop a tag.
- Onion skin: previous/next count, tint, opacity.
- Preview panel (described in section 5).
- Export: GIF, spritesheet (rows / columns / packed, padding, trim) + JSON metadata in the Aseprite-style layout so game engines' existing importers can read it; PNG sequence.

**Done when:** a multi-layer, tagged animation round-trips through `.acms` and exports a correct GIF and spritesheet + JSON.

### Phase 6 — Game-dev features
- Tilesets and tilemap layers. Tile stamp tool, tile picker panel.
- Two edit modes: *stamp* (place existing tiles) and *auto* (drawing pixels on the map edits the tile, so every instance updates; new tiles are created when needed, duplicates merged).
- Slices with optional 9-slice centre and pivot; exported in the JSON.
- Export: tileset PNG, tilemap as Tiled `.tmj` and CSV.

**Done when:** editing one tile updates all its instances, and the exported `.tmj` opens in Tiled.

### Phase 7 — Scripting
- Lua host in `acms-script`. Scripts folder in the user config directory, plus bundled examples. Reload without restarting.
- API (keep it small and documented):
  - `app.command(id, params)`, `app.active_sprite`, `app.active_layer` / `active_frame` / `active_cel`, `app.fg_color` / `bg_color`
  - `sprite`: size, layers, frames, palette, selection, tags; `cel:get_pixel` / `set_pixel` / `image` / `set_image`
  - `app.transaction(name, fn)` — everything inside is one undo step
  - `register_command{ id, name, run }`, `register_filter{...}`, `register_tool{ on_down, on_move, on_up }`
  - a small dialog builder (number, colour, text, checkbox, dropdown) for script parameters
- Sandbox: no `os`, no `io`, no `debug`, no loading native modules. File access only through an explicit, user-confirmed API (`app.choose_folder`, then file names inside that folder). A script is stopped after 10 seconds or 256 MB.
- A file in the user's scripts folder with the name of a bundled example replaces the example.
- Script console panel (REPL + output), errors shown with file and line, a script can never crash the app.
- Bundled examples: outline filter, palette swap, mirror sprite, "export every tag as separate GIF", a custom spray tool.

**Done when:** each example script works, appears in the command palette, and undoes as a single step.

### Phase 8 — Polish
- Preferences dialog, keymap editor, layout presets.
- Autosave and crash recovery, recent files, drag-and-drop to open.
- Import `.aseprite` / `.ase` files (find and evaluate an existing crate first; do not write a parser if a good one exists).
- **Timelapse export:** replay the undo history to a GIF.
- Hi-DPI check, light theme check, empty states, tooltips everywhere.
- Release builds for Windows and Linux; README with screenshots; user docs for the scripting API.

### Later / optional
True indexed colour mode with palette-based colour cycling, more blend modes, command-line batch export (`acms export in.acms --sheet out.png`), plugin packages, macOS build.

---

## 7. Testing

- **Core unit tests** for the document model, transactions (do → undo → redo returns identical data), compositor, each stroke-pipeline stage.
- **Golden image tests:** run a scripted stroke through a tool and compare with a stored PNG in `tests/golden/`. Every tool and ink gets at least one.
- **Round-trip tests:** save → load → compare for `.acms`; export → decode for GIF and spritesheets.
- **Script tests:** run each bundled Lua script headless against a test sprite.
- UI is tested by hand; keep UI code thin so there is little to test there.

---

## 8. Working rules for Claude Code

1. Read `PLAN.md` and `PROGRESS.md` at the start of every session. Work on one phase at a time; do not start the next phase until the current one meets its "Done when".
2. Before each phase, write a short task list for it in `PROGRESS.md`, then implement.
3. Keep `acms-core` free of UI and file IO. If you feel the need to import egui there, the design is wrong — stop and rethink.
4. No document change outside a transaction. No UI action that is not a command.
5. Small commits with clear messages. Run `cargo fmt`, `cargo clippy -- -D warnings` and `cargo test` before each commit.
6. Check current crate versions and docs instead of guessing APIs. Ask before adding a heavy or unusual dependency.
7. No `unwrap()` / `expect()` on anything that depends on user input or files. No `unsafe` without a comment explaining why.
8. When something in this plan turns out to be wrong or impractical, say so, propose the change, and update this file after it is agreed — do not silently go another way.
9. Do not add features that are not in this plan. Put ideas under "Ideas" in `PROGRESS.md` instead.
10. After every phase, run the app and describe what to click to verify it, so the result can be checked by hand.

### First prompt to use

> Read PLAN.md. Create the workspace and do Phase 0 only. Create PROGRESS.md with the Phase 0 task list first, then implement. Stop when Phase 0's "Done when" is met and tell me how to verify it.
