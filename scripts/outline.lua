-- Outline: draws a one-pixel line in the foreground colour around everything on the cel.
--
-- A filter is given a copy of the picture of the active cel. Whatever it does to that copy
-- is written back as one undo step, inside the selection if there is one.

register_filter{
  id = "example.outline",
  name = "Outline",
  run = function(image)
    local color = app.fg_color
    local width, height = image.width, image.height

    -- Look at every pixel once, before anything is drawn.
    local solid = {}
    for y = 0, height - 1 do
      for x = 0, width - 1 do
        solid[y * width + x] = image:get_pixel(x, y).a > 0
      end
    end
    local function is_solid(x, y)
      return x >= 0 and y >= 0 and x < width and y < height and solid[y * width + x]
    end

    for y = 0, height - 1 do
      for x = 0, width - 1 do
        if not is_solid(x, y)
          and (is_solid(x - 1, y) or is_solid(x + 1, y) or is_solid(x, y - 1) or is_solid(x, y + 1))
        then
          image:set_pixel(x, y, color)
        end
      end
    end
  end,
}
