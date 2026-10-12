-- Palette Swap: replaces one colour with another on every image layer.
--
-- Shows how to ask for values with a dialog, and how to change many cels as one undo step.

register_command{
  id = "example.palette_swap",
  name = "Palette Swap…",
  run = function()
    local sprite = app.active_sprite
    if not sprite then
      error("No sprite is open.", 0)
    end

    local answer = Dialog("Palette Swap")
      :color{ id = "from", label = "Replace", value = app.fg_color }
      :color{ id = "to", label = "With", value = app.bg_color }
      :check{ id = "both", label = "Swap both ways", value = false }
      :dropdown{ id = "frames", label = "Frames", options = { "Every frame", "This frame" } }
      :show()
    if not answer then
      return
    end

    local first, last = 1, #sprite.frames
    if answer.frames == "This frame" then
      first, last = app.active_frame, app.active_frame
    end

    local changed = 0
    app.transaction("Palette Swap", function()
      -- Every picture is read before any is written: linked cels share one picture, and
      -- writing as we go would change it once for every frame it is linked to.
      local work = {}
      for _, layer in ipairs(sprite.layers) do
        if layer.kind == "image" and not layer.locked then
          for frame = first, last do
            local cel = layer:cel(frame)
            local image = cel:image()
            local touched = false
            for y = 0, image.height - 1 do
              for x = 0, image.width - 1 do
                local pixel = image:get_pixel(x, y)
                if pixel == answer.from then
                  image:set_pixel(x, y, answer.to)
                  touched = true
                  changed = changed + 1
                elseif answer.both and pixel == answer.to then
                  image:set_pixel(x, y, answer.from)
                  touched = true
                  changed = changed + 1
                end
              end
            end
            if touched then
              work[#work + 1] = { cel = cel, image = image }
            end
          end
        end
      end
      for _, item in ipairs(work) do
        item.cel:set_image(item.image)
      end
    end)
    print("Palette Swap: " .. changed .. " pixels changed")
  end,
}
