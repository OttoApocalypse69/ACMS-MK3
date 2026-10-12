-- The part of the scripting API that is written in Lua. It runs once in every new Lua state
-- and returns the functions the Rust side needs. Everything that touches the application goes
-- through `H`, which calls the host function that is current for this entry into Lua.

local current

-- Calls the host. A refused request comes back as `false, message`, where the message
-- already names the line of the script that made the call, and is raised here.
local function answered(ok, ...)
  if not ok then
    error((...), 0)
  end
  return ...
end

local function H(op, ...)
  if not current then
    error("the application is not available right now", 3)
  end
  return answered(current(op, ...))
end

local function read_only(what)
  return function(_, key)
    error(what .. "." .. tostring(key) .. " cannot be set", 2)
  end
end

-- Colours ---------------------------------------------------------------------------------

local Color = {}
Color.__index = Color
Color.__eq = function(a, b)
  return a.r == b.r and a.g == b.g and a.b == b.b and a.a == b.a
end
Color.__tostring = function(c)
  if c.a == 255 then
    return string.format("#%02x%02x%02x", c.r, c.g, c.b)
  end
  return string.format("#%02x%02x%02x%02x", c.r, c.g, c.b, c.a)
end
function Color:hex()
  return tostring(self)
end

-- Cels ------------------------------------------------------------------------------------

local Cel = {}
Cel.__index = Cel
Cel.__tostring = function(cel)
  return "Cel(layer " .. cel.layer .. ", frame " .. cel.frame .. ")"
end
function Cel:get_pixel(x, y)
  return H("get_pixel", self.layer, self.frame, x, y)
end
function Cel:set_pixel(x, y, color)
  H("set_pixel", self.layer, self.frame, x, y, color)
end
function Cel:image()
  return H("image", self.layer, self.frame)
end
function Cel:set_image(image)
  H("set_image", self.layer, self.frame, image)
end

-- Layers ----------------------------------------------------------------------------------

local Layer = {}
Layer.__index = Layer
Layer.__tostring = function(layer)
  return "Layer(" .. layer.name .. ")"
end
function Layer:cel(frame)
  if self.kind ~= "image" and self.kind ~= "tilemap" then
    return nil
  end
  if frame == nil then
    frame = H("active_frame")
  end
  return setmetatable({ layer = self.id, frame = frame }, Cel)
end

local function as_layers(list)
  for _, layer in ipairs(list) do
    setmetatable(layer, Layer)
  end
  return list
end

-- The sprite ------------------------------------------------------------------------------

local Selection = {}
Selection.__index = Selection
function Selection:contains(x, y)
  return H("selected", x, y)
end

-- There is one sprite object: it always means the sprite that is active right now.
local sprite = setmetatable({}, {
  __index = function(_, key)
    if key == "width" then
      return (H("size"))
    elseif key == "height" then
      local _, height = H("size")
      return height
    elseif key == "layers" then
      return as_layers(H("layers"))
    elseif key == "frames" then
      return H("frames")
    elseif key == "palette" then
      return H("palette")
    elseif key == "tags" then
      return H("tags")
    elseif key == "selection" then
      local selection = H("selection")
      return selection and setmetatable(selection, Selection)
    end
    return nil
  end,
  __newindex = read_only("sprite"),
  __tostring = function()
    local width, height = H("size")
    return "Sprite(" .. width .. " x " .. height .. ")"
  end,
})

-- Registered commands, filters and tools ---------------------------------------------------

local commands, commands_by_id = {}, {}
local tools, tools_by_id = {}, {}

local function check_definition(def, what, taken)
  if type(def) ~= "table" then
    error(what .. " takes a table", 3)
  end
  if type(def.id) ~= "string" or def.id == "" then
    error(what .. " needs an id", 3)
  end
  if def.name ~= nil and type(def.name) ~= "string" then
    error(what .. ": the name must be text", 3)
  end
  if taken[def.id] then
    error(what .. ": `" .. def.id .. "` is registered already", 3)
  end
end

local function add_command(id, name, run)
  local entry = { id = id, name = name or id, run = run }
  commands[#commands + 1] = entry
  commands_by_id[id] = entry
end

-- The application ---------------------------------------------------------------------------

local app_functions = {}

function app_functions.command(id, params)
  if type(id) ~= "string" then
    error("app.command needs the id of a command", 2)
  end
  local own = commands_by_id[id]
  if own then
    own.run()
    return
  end
  H("command", id, params)
end

function app_functions.transaction(name, fn)
  if type(name) == "function" then
    name, fn = "Script", name
  end
  if type(name) ~= "string" or type(fn) ~= "function" then
    error("app.transaction needs a name and a function", 2)
  end
  H("begin")
  local ok, err = pcall(fn)
  if ok then
    H("commit", name)
  else
    H("rollback")
    error(err, 0)
  end
end

function app_functions.choose_folder(title)
  return H("choose_folder", title)
end

function app_functions.join_path(folder, name)
  return H("join_path", folder, name)
end

local app = setmetatable({}, {
  __index = function(_, key)
    local fn = app_functions[key]
    if fn then
      return fn
    elseif key == "active_sprite" then
      if H("has_sprite") then
        return sprite
      end
      return nil
    elseif key == "fg_color" then
      return H("fg")
    elseif key == "bg_color" then
      return H("bg")
    elseif key == "active_frame" then
      if H("has_sprite") then
        return H("active_frame")
      end
      return nil
    elseif key == "active_layer" then
      local layer = H("active_layer")
      return layer and setmetatable(layer, Layer)
    elseif key == "active_cel" then
      local layer = H("active_layer")
      return layer and Layer.cel(setmetatable(layer, Layer))
    end
    return nil
  end,
  __newindex = function(_, key, value)
    if key == "fg_color" then
      H("set_fg", value)
    elseif key == "bg_color" then
      H("set_bg", value)
    else
      error("app." .. tostring(key) .. " cannot be set", 2)
    end
  end,
})

local function register_command(def)
  check_definition(def, "register_command", commands_by_id)
  if type(def.run) ~= "function" then
    error("register_command needs a run function", 2)
  end
  add_command(def.id, def.name, def.run)
end

-- A filter changes the picture of the active cel: `run` is given a copy of it, changes that
-- copy (or returns another image), and the result is written back as one undo step.
local function register_filter(def)
  check_definition(def, "register_filter", commands_by_id)
  if type(def.run) ~= "function" then
    error("register_filter needs a run function", 2)
  end
  local name = def.name or def.id
  add_command(def.id, name, function()
    local cel = app.active_cel
    if not cel then
      error("There is no cel to work on: open a sprite and select an image layer.", 0)
    end
    app_functions.transaction(name, function()
      local image = cel:image()
      local result = def.run(image)
      if result ~= nil then
        image = result
      end
      cel:set_image(image)
    end)
  end)
end

local function register_tool(def)
  check_definition(def, "register_tool", tools_by_id)
  for _, key in ipairs({ "on_down", "on_move", "on_up" }) do
    if def[key] ~= nil and type(def[key]) ~= "function" then
      error("register_tool: " .. key .. " must be a function", 2)
    end
  end
  local entry = {
    id = def.id,
    name = def.name or def.id,
    down = def.on_down,
    move = def.on_move,
    up = def.on_up,
  }
  tools[#tools + 1] = entry
  tools_by_id[def.id] = entry
end

-- Dialogs -----------------------------------------------------------------------------------

-- What a script yields to have a dialog shown.
local DIALOG = {}

local Dialog = {}
Dialog.__index = Dialog

local function field(kind)
  return function(self, def)
    if type(def) ~= "table" or type(def.id) ~= "string" then
      error("a dialog field needs an id", 2)
    end
    self.fields[#self.fields + 1] = {
      kind = kind,
      id = def.id,
      label = def.label,
      value = def.value,
      min = def.min,
      max = def.max,
      decimals = def.decimals,
      options = def.options,
    }
    return self
  end
end

Dialog.number = field("number")
Dialog.color = field("color")
Dialog.text = field("text")
Dialog.check = field("check")
Dialog.dropdown = field("dropdown")

function Dialog:show()
  if not coroutine.isyieldable() or not H("can_ask") then
    error("a dialog cannot be shown here", 2)
  end
  H("check_dialog", self)
  return coroutine.yield(DIALOG, self)
end

local function new_dialog(title)
  if title ~= nil and type(title) ~= "string" then
    error("Dialog takes a title", 2)
  end
  return setmetatable({ title = title or "Script", fields = {} }, Dialog)
end

-- Globals -----------------------------------------------------------------------------------

function print(...)
  local parts = {}
  for i = 1, select("#", ...) do
    parts[i] = tostring((select(i, ...)))
  end
  H("print", table.concat(parts, "\t"))
end

_G.app = app
_G.Dialog = new_dialog
_G.register_command = register_command
_G.register_filter = register_filter
_G.register_tool = register_tool

-- No way to load code or files from inside a script.
_G.dofile = nil
_G.loadfile = nil
_G.load = nil
_G.collectgarbage = nil
string.dump = nil

-- For the Rust side -------------------------------------------------------------------------

local internal = { color = Color, dialog = DIALOG, commands = commands, tools = tools }

function internal.set_host(fn)
  current = fn
end

function internal.run_command(id)
  local command = commands_by_id[id]
  if not command then
    error("there is no script command `" .. tostring(id) .. "`", 0)
  end
  command.run()
end

function internal.tool_event(id, phase, event)
  local tool = tools_by_id[id]
  local handler = tool and tool[phase]
  if handler then
    handler(event)
  end
end

-- Forgets what was registered after the first `command_count` commands and `tool_count`
-- tools: a script that fails while loading leaves nothing behind.
function internal.forget_after(command_count, tool_count)
  for i = #commands, command_count + 1, -1 do
    commands_by_id[commands[i].id] = nil
    commands[i] = nil
  end
  for i = #tools, tool_count + 1, -1 do
    tools_by_id[tools[i].id] = nil
    tools[i] = nil
  end
end

-- A new environment for one script: it reads the globals and keeps its own to itself.
function internal.environment()
  return setmetatable({}, { __index = _G })
end

-- Runs a line typed into the console and shows what it evaluated to, if anything.
local function show(...)
  if select("#", ...) > 0 then
    print(...)
  end
end

function internal.eval(chunk)
  show(chunk())
end

return internal
