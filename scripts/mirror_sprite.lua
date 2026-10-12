-- Mirror Sprite: flips every image layer left to right, in every frame.
--
-- Shows how to work on all cels of a sprite as one undo step.

register_command{
  id = "example.mirror_sprite",
  name = "Mirror Sprite",
  run = function()
    local sprite = app.active_sprite
    if not sprite then
      error("No sprite is open.", 0)
    end

    app.transaction("Mirror Sprite", function()
      -- Every picture is read before any is written: linked cels share one picture, and
      -- flipping as we go would flip it once for every frame it is linked to.
      local work = {}
      for _, layer in ipairs(sprite.layers) do
        if layer.kind == "image" and not layer.locked then
          for frame = 1, #sprite.frames do
            local cel = layer:cel(frame)
            local image = cel:image()
            local flipped = image:clone()
            for y = 0, image.height - 1 do
              for x = 0, image.width - 1 do
                flipped:set_pixel(image.width - 1 - x, y, image:get_pixel(x, y))
              end
            end
            work[#work + 1] = { cel = cel, image = flipped }
          end
        end
      end
      for _, item in ipairs(work) do
        item.cel:set_image(item.image)
      end
    end)
  end,
}
