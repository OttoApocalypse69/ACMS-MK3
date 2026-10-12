-- Spray: a tool that scatters dots of the drawing colour around the pointer.
--
-- A tool is told when the button goes down, when the pointer moves with the button held,
-- and when the button is released. Everything it draws from press to release is one undo
-- step.

local RADIUS = 4 -- how far from the pointer a dot can land, in pixels
local DOTS = 6 -- how many dots every event scatters

local function spray(event)
  local cel = app.active_cel
  if not cel then
    return
  end
  for _ = 1, DOTS do
    -- A point picked evenly from a disc around the pointer.
    local angle = math.random() * 2 * math.pi
    local distance = math.sqrt(math.random()) * RADIUS
    local x = event.x + 0.5 + math.cos(angle) * distance
    local y = event.y + 0.5 + math.sin(angle) * distance
    -- event.color is the foreground colour for the primary button, the background colour
    -- for the secondary one.
    cel:set_pixel(x, y, event.color)
  end
end

register_tool{
  id = "spray",
  name = "Spray",
  on_down = spray,
  on_move = spray,
}
