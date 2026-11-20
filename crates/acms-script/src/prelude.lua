-- The part of the scripting API that is written in Lua. It runs once in every new Lua state
-- and returns the functions the Rust side needs. Everything that touches the application goes
-- through `H`, which calls the host function that is current for this entry into Lua.
--
-- Scripts share this state, so they must not be able to change what another one sees: each
-- gets its own globals (see `internal.environment`), every table of the API has its metatable
-- locked, and this file keeps its own copies of the library functions it uses.

local error, pcall, xpcall, select, type, tostring = error, pcall, xpcall, select, type, tostring
local setmetatable, getmetatable, ipairs, pairs = setmetatable, getmetatable, ipairs, pairs
local rawget = rawget
local concat, remove = table.concat, table.remove
local format, byte, sub = string.format, string.byte, string.sub
local yield, isyieldable, resume = coroutine.yield, coroutine.isyieldable, coroutine.resume

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
    return format("#%02x%02x%02x", c.r, c.g, c.b)
  end
  return format("#%02x%02x%02x%02x", c.r, c.g, c.b, c.a)
end
Color.__metatable = false
function Color:hex()
  return tostring(self)
end

-- Cels ------------------------------------------------------------------------------------

local Cel = {}
Cel.__index = Cel
Cel.__metatable = false
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
Layer.__metatable = false
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
Selection.__metatable = false
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
  __metatable = false,
})

-- Registered commands, filters and tools ---------------------------------------------------

local commands, commands_by_id = {}, {}
local tools, tools_by_id = {}, {}
-- The ids of the application's own commands: no script may take one, so that
-- `app.command` with such an id always means the application's command.
local reserved = {}

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
  if taken == commands_by_id and reserved[def.id] then
    error(what .. ": `" .. def.id .. "` is a command of the application", 3)
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
  local own = not reserved[id] and commands_by_id[id]
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
  __metatable = false,
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
Dialog.__metatable = false

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
  if not isyieldable() or not H("can_ask") then
    error("a dialog cannot be shown here", 2)
  end
  H("check_dialog", self)
  return yield(DIALOG, self)
end

local function new_dialog(title)
  if title ~= nil and type(title) ~= "string" then
    error("Dialog takes a title", 2)
  end
  return setmetatable({ title = title or "Script", fields = {} }, Dialog)
end

-- Globals -----------------------------------------------------------------------------------

local function print(...)
  local parts = {}
  for i = 1, select("#", ...) do
    parts[i] = tostring((select(i, ...)))
  end
  H("print", concat(parts, "\t"))
end

-- For the Rust side -------------------------------------------------------------------------

local internal = { color = Color, dialog = DIALOG, commands = commands, tools = tools }

-- Once a script's time is up, catching the error does not help: whatever caught it raises it
-- again. (`internal.expired` and `internal.too_long` are set by the Rust side.)
local function passed_on(...)
  if internal.expired() then
    error(internal.too_long, 0)
  end
  return ...
end

local function safe_pcall(fn, ...)
  return passed_on(pcall(fn, ...))
end

local function safe_xpcall(fn, handler, ...)
  return passed_on(xpcall(fn, handler, ...))
end

local function safe_resume(co, ...)
  return passed_on(resume(co, ...))
end

-- A finaliser would run whenever memory is collected, outside the script's own time.
local function safe_setmetatable(t, mt)
  if type(mt) == "table" and rawget(mt, "__gc") ~= nil then
    error("__gc is not available to scripts", 2)
  end
  return setmetatable(t, mt)
end

-- Matching runs inside Lua's C code, where the time limit cannot stop it. So how much work a
-- call can be is estimated first, and a call that could hold the application up for more than
-- a moment is refused. Every call is counted: a plain search too, since it compares the
-- pattern at every place in the text.
local MATCH_WORK = 1e8

local function check_match(subject, pattern, plain, level)
  if type(pattern) == "number" then
    pattern = tostring(pattern)
  end
  if type(subject) == "number" then
    subject = tostring(subject)
  end
  if type(subject) ~= "string" or type(pattern) ~= "string" then
    return -- the original function says what is wrong
  end
  local length, n = #subject, #pattern
  local repeats = 0
  if not plain then
    local i = 1
    while i <= n do
      local c = byte(pattern, i)
      if c == 37 then -- `%x`
        -- `%bxy` scans to the end of the text from where it starts: as costly as a repeat.
        if byte(pattern, i + 1) == 98 then
          repeats = repeats + 1
          i = i + 4
        else
          i = i + 2
        end
      else
        if c == 42 or c == 43 or c == 45 or c == 63 then -- `*`, `+`, `-`, `?`
          repeats = repeats + 1
        end
        i = i + 1
      end
    end
  end
  local anchored = not plain and byte(pattern, 1) == 94 -- `^`
  -- The places a match can start, the pattern compared at each, and the ways the repeats
  -- can share out the text.
  local work = (anchored and 1 or length + 1) * (n + 1)
  for k = 1, repeats do
    work = work * (length + k) / k
    if work > MATCH_WORK then
      break
    end
  end
  if work > MATCH_WORK then
    error("this pattern is too costly for text this long; use a simpler pattern or shorter text",
      level)
  end
end

local guarded_string = {}
for name, fn in pairs(string) do
  guarded_string[name] = fn
end
string.dump = nil
guarded_string.dump = nil
for _, name in ipairs({ "find", "match", "gmatch", "gsub" }) do
  local original = string[name]
  local plain_at = name == "find" and 4 or nil
  local guarded = function(subject, pattern, ...)
    local plain = plain_at and select(2, ...)
    check_match(subject, pattern, plain, 3)
    return original(subject, pattern, ...)
  end
  string[name] = guarded
  guarded_string[name] = guarded
end

-- The metatable of strings is shared by every script: it is locked, so `("x"):upper()` always
-- means the real `string.upper`.
getmetatable("").__metatable = false

-- What every script finds among its globals. The libraries are copies, so a script that
-- changes `string.format` changes its own.
local base_names = {
  "assert", "error", "ipairs", "next", "pairs", "rawequal", "rawget", "rawlen", "select",
  "getmetatable", "tonumber", "tostring", "type", "_VERSION",
}
local library_names = { "table", "math", "utf8", "coroutine" }

-- Called once by the Rust side, after `Color` has been added: from then on the globals of
-- the state itself are no longer what scripts see.
function internal.seal()
  local G = _G
  local api = {
    app = app,
    Dialog = new_dialog,
    Color = G.Color,
    register_command = register_command,
    register_filter = register_filter,
    register_tool = register_tool,
    print = print,
    pcall = safe_pcall,
    xpcall = safe_xpcall,
    setmetatable = safe_setmetatable,
  }
  internal.api = api
  internal.base = {}
  for _, name in ipairs(base_names) do
    internal.base[name] = G[name]
  end
  -- No way to load code or files, or to change tables behind their metatables.
  G.dofile, G.loadfile, G.load, G.collectgarbage, G.rawset = nil, nil, nil, nil, nil
end

function internal.set_host(fn)
  current = fn
end

function internal.reserve(id)
  reserved[id] = true
end

-- Takes back a command the application could not add, so that it is not run in its place.
function internal.forget_command(id)
  if commands_by_id[id] then
    commands_by_id[id] = nil
    for i = #commands, 1, -1 do
      if commands[i].id == id then
        remove(commands, i)
      end
    end
  end
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

-- A new environment for one script: its own globals, which no other script sees.
function internal.environment()
  local env = {}
  for name, value in pairs(internal.base) do
    env[name] = value
  end
  for name, value in pairs(internal.api) do
    env[name] = value
  end
  for _, name in ipairs(library_names) do
    local copy = {}
    for key, value in pairs(_G[name]) do
      copy[key] = value
    end
    env[name] = copy
  end
  local strings = {}
  for key, value in pairs(guarded_string) do
    strings[key] = value
  end
  env.string = strings
  env.coroutine.resume = safe_resume
  env._G = env
  return env
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
