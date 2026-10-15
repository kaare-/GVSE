-- Create a sprite from a resolved job JSON (path in app.params.job).
-- Invoked headlessly: aseprite -b --script-param job=... --script create_from_spec.lua
-- Official refs: https://www.aseprite.org/docs/cli/  https://www.aseprite.org/api/

local function die(msg)
  error("gvse-art: " .. tostring(msg), 0)
end

local function read_file(path)
  local f, err = io.open(path, "r")
  if not f then
    die("cannot open job file: " .. tostring(err))
  end
  local data = f:read("*a")
  f:close()
  return data
end

local function parse_hex(color)
  if type(color) ~= "string" then
    die("color must be string hex")
  end
  local hex = color:gsub("#", "")
  local r, g, b, a
  if #hex == 6 then
    r = tonumber(hex:sub(1, 2), 16)
    g = tonumber(hex:sub(3, 4), 16)
    b = tonumber(hex:sub(5, 6), 16)
    a = 255
  elseif #hex == 8 then
    r = tonumber(hex:sub(1, 2), 16)
    g = tonumber(hex:sub(3, 4), 16)
    b = tonumber(hex:sub(5, 6), 16)
    a = tonumber(hex:sub(7, 8), 16)
  else
    die("invalid hex color: " .. color)
  end
  return r, g, b, a
end

local job_path = app.params["job"]
if not job_path or job_path == "" then
  die("missing --script-param job=<path>")
end

if not json or not json.decode then
  die("Aseprite json.decode unavailable; need a recent Aseprite build")
end

local job = json.decode(read_file(job_path))
local width = assert(job.width, "job.width required")
local height = assert(job.height, "job.height required")
local frames = assert(job.frames, "job.frames required")
local out_path = job.source_aseprite
if not out_path or out_path == "" then
  die("job.source_aseprite required")
end

local spr = Sprite(width, height, ColorMode.RGB)
spr.filename = out_path

local layer = spr.layers[1]
layer.name = job.layer or "organism"

local function ensure_frame(index)
  while #spr.frames < index do
    spr:newEmptyFrame(#spr.frames + 1)
  end
  return spr.frames[index]
end

for i, fr in ipairs(frames) do
  local frame = ensure_frame(i)
  -- Frame.duration is seconds.
  frame.duration = (fr.duration_ms or 100) / 1000.0
  local img = Image(ImageSpec{ width=width, height=height, colorMode=ColorMode.RGB })
  if job.transparent_background ~= false then
    img:clear()
  end
  for _, px in ipairs(fr.pixels or {}) do
    local r, g, b, a = parse_hex(px.color)
    img:drawPixel(px.x, px.y, app.pixelColor.rgba(r, g, b, a))
  end
  local existing = layer:cel(frame)
  if existing then
    existing.image = img
    existing.position = Point(0, 0)
  else
    spr:newCel(layer, frame, img, Point(0, 0))
  end
end

-- Drop trailing default empty frames if the job has fewer (should not happen).
while #spr.frames > #frames do
  spr:deleteFrame(#spr.frames)
end

if #frames > 1 then
  local tag = spr:newTag(1, #frames)
  tag.name = job.tag or "idle"
  tag.aniDir = AniDir.FORWARD
end

if job.palette then
  local colors = {}
  for _, hex in pairs(job.palette) do
    local r, g, b, a = parse_hex(hex)
    colors[#colors + 1] = Color{ r=r, g=g, b=b, a=a }
  end
  if #colors > 0 then
    local pal = Palette(#colors)
    for i, c in ipairs(colors) do
      pal:setColor(i - 1, c)
    end
    spr:setPalette(pal)
  end
end

app.fs.makeDirectory(app.fs.filePath(out_path))
spr:saveAs(out_path)
print("wrote " .. out_path)
