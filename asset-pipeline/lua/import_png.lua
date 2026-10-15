-- Import a PNG into a native .aseprite document with optional resize / palette.
-- Params: input, output, width?, height?, palette? (JSON path), layer?

local function die(msg)
  error("gvse-art: " .. tostring(msg), 0)
end

local function read_file(path)
  local f, err = io.open(path, "r")
  if not f then
    die("cannot open: " .. tostring(path) .. " (" .. tostring(err) .. ")")
  end
  local data = f:read("*a")
  f:close()
  return data
end

local input = app.params["input"]
local output = app.params["output"]
if not input or input == "" then die("missing --script-param input=") end
if not output or output == "" then die("missing --script-param output=") end

local spr = app.open(input)
if not spr then
  die("failed to open " .. input)
end

local want_w = tonumber(app.params["width"] or "")
local want_h = tonumber(app.params["height"] or "")
if want_w and want_h then
  if spr.width ~= want_w or spr.height ~= want_h then
    spr:resize(want_w, want_h)
  end
end

local palette_path = app.params["palette"]
if palette_path and palette_path ~= "" then
  if not json or not json.decode then
    die("json.decode unavailable")
  end
  local payload = json.decode(read_file(palette_path))
  local colors = payload.colors or payload
  if type(colors) ~= "table" then
    die("palette JSON must contain colors array")
  end
  local parsed = {}
  for _, hex in ipairs(colors) do
    local h = tostring(hex):gsub("#", "")
    if #h ~= 6 then
      die("bad palette color " .. tostring(hex))
    end
    parsed[#parsed + 1] = Color{
      r = tonumber(h:sub(1, 2), 16),
      g = tonumber(h:sub(3, 4), 16),
      b = tonumber(h:sub(5, 6), 16),
      a = 255,
    }
  end
  if #parsed > 0 then
    local pal = Palette(#parsed)
    for i, c in ipairs(parsed) do
      pal:setColor(i - 1, c)
    end
    spr:setPalette(pal)
    if payload.color_mode == "indexed" then
      app.command.ChangePixelFormat{ format="indexed", dithering="none" }
    end
  end
end

if spr.layers[1] then
  spr.layers[1].name = app.params["layer"] or "imported"
end

app.fs.makeDirectory(app.fs.filePath(output))
spr:saveAs(output)
print("imported " .. input .. " -> " .. output)
