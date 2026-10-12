-- Export Every Tag as a GIF: writes one GIF per tag into a folder you choose.
--
-- Shows how a script gets at files: it asks for a folder with app.choose_folder, and may
-- then name files inside that folder (and nowhere else) when it runs a command.

register_command{
  id = "example.export_tags",
  name = "Export Every Tag as a GIF…",
  run = function()
    local sprite = app.active_sprite
    if not sprite then
      error("No sprite is open.", 0)
    end
    local tags = sprite.tags
    if #tags == 0 then
      error("This sprite has no tags.", 0)
    end

    local folder = app.choose_folder("Export the tags to")
    if not folder then
      return
    end

    local used = {}
    for index, tag in ipairs(tags) do
      -- A tag can be called anything; a file cannot.
      local name = tag.name:gsub("[^%w%-_ ]", "_")
      if name == "" then
        name = "tag"
      end
      if used[name] then
        name = name .. "_" .. index
      end
      used[name] = true
      -- Commands count tags and frames from 0.
      app.command("file.export_gif", {
        path = app.join_path(folder, name .. ".gif"),
        tag = index - 1,
      })
    end
    print("Exported " .. #tags .. " GIFs to " .. folder)
  end,
}
