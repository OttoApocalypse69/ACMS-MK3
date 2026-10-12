# ACMS scripting

Scripts are written in Lua 5.4. A script can add commands, filters and tools to ACMS. What a
script adds shows up next to the built-in things: in the command palette, in the Scripts menu,
in the Tools panel, and it can be given a shortcut in the keymap file.

## Where scripts live

- The examples in this folder come with the application and are always loaded.
- Your own scripts go in the `scripts` folder of the configuration directory. The Script
  Console names the folder when the application starts. On Windows it is
  `%APPDATA%\acms-mk3\config\scripts`, on Linux `~/.config/acms-mk3/scripts`.
- Every file ending in `.lua` in that folder is loaded, in order of name.
- A file of yours with the same name as an example (`outline.lua`, say) replaces the example.
- **Scripts → Reload Scripts** loads everything again without restarting. Do this after you
  change a file.

Each file has its own global variables. A variable you set in one script is not visible in
another.

## The Script Console

The Script Console panel shows what scripts print and what went wrong, with the file and line.
The line at the bottom runs Lua: type an expression to see its value, or a statement to run it.
Variables you set there stay until the scripts are reloaded.

```lua
app.active_sprite.width
app.fg_color = "#ff8800"
for i, layer in ipairs(app.active_sprite.layers) do print(i, layer.name, layer.kind) end
```

## What a script cannot do

Scripts are sandboxed. They have Lua's `string`, `table`, `math`, `utf8` and `coroutine`
libraries. They do not have `os`, `io`, `debug`, `package`, `require`, `load`, `loadfile` or
`dofile`, and cannot load native modules.

- **Files:** a script can only name files inside a folder you picked for it; see
  `app.choose_folder`.
- **Time:** a script that runs for more than 10 seconds is stopped. Time you spend in a dialog
  or a file dialog does not count.
- **Memory:** a script that uses more than 256 MB is stopped.

An error in a script never takes the application down: the script stops, the error is shown,
and whatever the script had changed inside a transaction is taken back.

## Counting

- Pixel positions count from 0. `(0, 0)` is the top left pixel of the canvas.
- Frames, layers and palette colours are Lua lists and count from 1.
- The parameters of commands are passed on as they are, and commands count frames and tags
  from 0. See "Commands" below.

## Colours

A colour has the fields `r`, `g`, `b` and `a`, each from 0 to 255.

```lua
local red = Color(255, 0, 0)        -- alpha is 255 if left out
local faint = Color(255, 0, 0, 64)
local teal = Color("#008080")       -- or "008080", or with alpha: "#00808080"
print(red.r, red == Color("#ff0000"), tostring(teal))
```

Wherever a colour is expected you can also give hex text: `cel:set_pixel(0, 0, "#ff0000")`.
Two colours are equal with `==` when all four fields are equal.

## `app`

| | |
|---|---|
| `app.active_sprite` | The sprite being edited, or `nil` if none is open. |
| `app.active_layer` | The selected layer, or `nil`. |
| `app.active_frame` | The number of the frame being shown, or `nil`. |
| `app.active_cel` | The cel of the selected layer in the frame being shown. `nil` if the selected layer is a group or a reference layer. |
| `app.fg_color`, `app.bg_color` | The primary and secondary colour. Both can be set. |
| `app.command(id, params)` | Runs a command. See "Commands". |
| `app.transaction(name, fn)` | Runs `fn`; everything it changes is one undo step called `name`. |
| `app.choose_folder(title)` | Asks the user for a folder. Returns its path, or `nil` if they cancel. |
| `app.join_path(folder, name)` | Puts a folder and a file name together. |

### Transactions

Every change a script makes to the sprite can be undone. Outside a transaction, each change is
an undo step of its own, so a script that sets 100 pixels leaves 100 steps. Wrap the work in a
transaction to make it one:

```lua
app.transaction("Diagonal", function()
  local cel = app.active_cel
  for i = 0, 7 do
    cel:set_pixel(i, i, app.fg_color)
  end
end)
```

- Commands run inside the transaction become part of the same undo step.
- If the function raises an error, everything it did is taken back and the error goes on.
- A transaction inside a transaction is part of the outer one.
- A dialog cannot be shown inside a transaction. Ask first, then change.

### Commands

`app.command(id, params)` runs any command of the application, the same ones the menus and
shortcuts run. `params` is a table of named values and can be left out.

```lua
app.command("layer.new")
app.command("brush.set_size", { size = 3 })
app.command("color.set_primary", { color = Color(255, 128, 0) })
app.command("frame.goto", { frame = 0 })   -- commands count frames from 0
```

The command palette lists the commands by name; the ids are the ones used in the keymap file.
A command that cannot run raises an error, which `pcall` can catch. A command that needs
something from the user, like a file name, opens its dialog after the script has finished.

A command registered by a script is run the same way: `app.command("example.outline")`.

### Files

A script cannot read or write files by itself. It can run the commands that do, and it may
give them a `path` only inside a folder the user has just picked:

```lua
local folder = app.choose_folder("Export to")
if folder then
  app.command("file.export_png", { path = app.join_path(folder, "sprite.png"), scale = 4 })
end
```

The permission lasts until the script finishes. A `path` anywhere else is refused.

## The sprite

`app.active_sprite` always means the sprite that is active right now. Its fields are read
only; change the sprite through cels, or with commands.

| | |
|---|---|
| `sprite.width`, `sprite.height` | The size of the canvas in pixels. |
| `sprite.layers` | The layers, bottom to top, groups before what is in them. |
| `sprite.frames` | The frames. Each has `duration`, in milliseconds. `#sprite.frames` is how many there are. |
| `sprite.palette` | The colours of the palette. |
| `sprite.tags` | The tags. Each has `name`, `from`, `to` (frame numbers), `direction` (`"forward"`, `"reverse"` or `"pingpong"`) and `color`. |
| `sprite.selection` | `nil` if nothing is selected. Otherwise `x`, `y`, `width`, `height` of the box around the selection, and `selection:contains(x, y)`. |

What you read is a copy made at that moment. Read it again after the sprite has changed.

### Layers

A layer has `id`, `name`, `kind` (`"image"`, `"group"`, `"tilemap"` or `"reference"`) and
`visible`. Image and tilemap layers also have `opacity` (0 to 255) and `locked`.

`layer:cel(frame)` is the cel of that layer in a frame; without a frame, in the frame being
shown. It is `nil` for groups and reference layers, which have no cels.

### Cels

A cel is the picture of one layer in one frame.

| | |
|---|---|
| `cel:get_pixel(x, y)` | The colour at a canvas position. Transparent where nothing is drawn, and outside the canvas. |
| `cel:set_pixel(x, y, color)` | Sets one pixel. Outside the canvas nothing happens. |
| `cel:image()` | A copy of the whole picture, as an image the size of the canvas. |
| `cel:set_image(image)` | Replaces the picture with an image, which must be the size of the canvas. |

Like the drawing tools, scripts only change pixels inside the selection, if there is one, and
a locked layer refuses changes. Positions with fractions are rounded down.

Three things to know:

- **Linked cels share one picture.** Changing one changes all of them. A script that goes
  through every frame should read all the pictures first and write them afterwards; see
  `mirror_sprite.lua`.
- **A tilemap layer is made of tiles.** Drawing on it changes the tile, and with it every
  place the tile is used.
- **`set_pixel` is for a few pixels.** To change a whole picture, get the image, change it,
  and set it once.

### Images

An image is a picture that belongs to the script. Changing it changes nothing in the sprite
until it is given to `cel:set_image`.

| | |
|---|---|
| `image.width`, `image.height` | |
| `image:get_pixel(x, y)` | Transparent outside the image. |
| `image:set_pixel(x, y, color)` | Does nothing outside the image. |
| `image:clone()` | A copy. |

## Adding a command

```lua
register_command{
  id = "my.clear_corner",
  name = "Clear the Corner",
  run = function()
    local cel = app.active_cel
    if not cel then
      error("Select an image layer first.", 0)
    end
    app.transaction("Clear the Corner", function()
      cel:set_pixel(0, 0, Color(0, 0, 0, 0))
    end)
  end,
}
```

- `id` must not be the id of another command. A prefix of your own keeps it apart.
- `name` is what the palette and the menu show.
- `error(message, 0)` stops the script and shows the message in a dialog. The `0` leaves the
  file and line out of it.
- Use a transaction if the command changes the sprite, so that it is one undo step.

## Adding a filter

A filter changes the picture of the active cel. `run` is given a copy of the picture. It
changes that copy, or returns another image. The result is written back as one undo step,
inside the selection if there is one. A filter is a command and is run like one.

```lua
register_filter{
  id = "my.invert",
  name = "Invert",
  run = function(image)
    for y = 0, image.height - 1 do
      for x = 0, image.width - 1 do
        local c = image:get_pixel(x, y)
        if c.a > 0 then
          image:set_pixel(x, y, Color(255 - c.r, 255 - c.g, 255 - c.b, c.a))
        end
      end
    end
  end,
}
```

## Adding a tool

A tool is told what the pointer does on the canvas. Everything it draws between the button
going down and coming up is one undo step, and Escape takes it back.

```lua
register_tool{
  id = "my_dots",
  name = "Dots",
  on_down = function(event) app.active_cel:set_pixel(event.x, event.y, event.color) end,
  on_move = function(event) app.active_cel:set_pixel(event.x, event.y, event.color) end,
  on_up = function(event) end,
}
```

Each of the three functions can be left out. The tool appears in the Tools panel and gets the
command `tool.<id>`. The `event` has:

| | |
|---|---|
| `x`, `y` | The canvas pixel under the pointer. It can lie outside the canvas. |
| `button` | `"primary"` or `"secondary"`: the button that started the stroke. |
| `color` | The primary colour for the primary button, the secondary colour for the other. |
| `shift`, `ctrl`, `alt` | Whether the key is held. |

A tool cannot show a dialog. If a tool raises an error, the stroke is taken back and the rest
of it is ignored.

## Dialogs

A command can ask for values before it does its work.

```lua
local answer = Dialog("Grid")
  :number{ id = "size", label = "Cell size", value = 8, min = 1, max = 64 }
  :color{ id = "ink", label = "Colour", value = app.fg_color }
  :text{ id = "name", label = "Name", value = "grid" }
  :check{ id = "dotted", label = "Dotted", value = false }
  :dropdown{ id = "where", label = "Draw on", options = { "This frame", "Every frame" } }
  :show()

if not answer then
  return   -- the user cancelled
end
print(answer.size, answer.ink, answer.name, answer.dotted, answer.where)
```

`show` waits until the user answers. It returns a table with one value for every `id`, or
`nil` if the dialog was cancelled.

| Field | Options | Value in the answer |
|---|---|---|
| `number` | `value`, `min`, `max`, `decimals` (0 if left out) | a whole number, or a number with fractions if `decimals` is more than 0 |
| `color` | `value` | a colour |
| `text` | `value` | text |
| `check` | `value` | `true` or `false` |
| `dropdown` | `options` (a list of text), `value` (one of them) | the chosen option |

Every field needs an `id`. `label` is the text shown next to it; the id is shown if it is left
out.

A dialog can be shown from a command and from the console. It cannot be shown while a script
file is loading, inside a transaction, in a filter's `run`, or from a tool.

## The examples

| File | What it adds | What it shows |
|---|---|---|
| `outline.lua` | Outline (filter) | reading and changing an image |
| `palette_swap.lua` | Palette Swap… (command) | a dialog, and changing every cel as one undo step |
| `mirror_sprite.lua` | Mirror Sprite (command) | working on all layers and frames, with linked cels |
| `export_tags.lua` | Export Every Tag as a GIF… (command) | asking for a folder and running export commands |
| `spray.lua` | Spray (tool) | a tool |
