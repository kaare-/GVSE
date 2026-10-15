-- Apply a palette JSON to a sprite and optionally convert to indexed.
-- Params: input, output, palette, color_mode?=rgb|indexed

local function die(msg)
  error("gvse-art: " .. tostring(msg), 0)
end

local function read_file(path)
  local f, err = io.open(path, "r")
  if not f then die(tostring(err)) end
  local data = f:read("*a")
  f:close()
  return data
end

local input = app.params["input"]
local output = app.params["output"]
local palette_path = app.params["palette"]
if not input or not output or not palette_path then
  die("need input, output, palette script params")
end

local spr = app.open(input)
if not spr then die("open failed: " .. input) end

local payload = json.decode(read_file(palette_path))
local colors = payload.colors or {}
local pal = Palette(#colors)
for i, hex in ipairs(colors) do
  local h = tostring(hex):gsub("#", "")
  pal:setColor(i - 1, Color{
    r = tonumber(h:sub(1, 2), 16),
    g = tonumber(h:sub(3, 4), 16),
    b = tonumber(h:sub(5, 6), 16),
    a = 255,
  })
end
spr:setPalette(pal)

local mode = app.params["color_mode"] or payload.color_mode or "rgb"
if mode == "indexed" then
  app.command.ChangePixelFormat{ format="indexed", dithering="none" }
end

app.fs.makeDirectory(app.fs.filePath(output))
spr:saveAs(output)
print("palette applied -> " .. output)
