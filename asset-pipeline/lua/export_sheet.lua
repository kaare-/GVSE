-- Optional Lua helper for sprite-sheet export.
-- Prefer CLI --sheet/--data from the Python runner for metadata fidelity.
-- https://www.aseprite.org/docs/cli/

local function die(msg)
  error("gvse-art: " .. tostring(msg), 0)
end

local input = app.params["input"]
local sheet = app.params["sheet"]
local data = app.params["data"]
if not input or not sheet then
  die("need input and sheet params")
end

local spr = app.open(input)
if not spr then die("open failed") end

app.command.ExportSpriteSheet{
  ui=false,
  type=SpriteSheetType.HORIZONTAL,
  textureFilename=sheet,
  dataFilename=data or "",
  dataFormat=SpriteSheetDataFormat.JSON_ARRAY,
  listLayers=true,
  listTags=true,
}

print("exported sheet " .. sheet)
