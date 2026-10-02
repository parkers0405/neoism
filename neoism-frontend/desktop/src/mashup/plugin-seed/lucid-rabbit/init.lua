local active_pack = false
pcall(function()
  local config = neoism.config.current()
  active_pack = type(config) == "table"
    and type(config.appearance) == "table"
    and config.appearance["mashup-pack"] == "lucid-blocks"
end)

-- Pack transitions rebuild the editor Lua manager against the candidate config.
-- Stay completely inert outside Lucid Blocks so this showcase has no idle cost.
if not active_pack then
  return
end

local last_revision = 0

local bird_polygons = {
  {
    color = "foreground",
    points = {
      {-0.56, 0.02, 0}, {-0.34, -0.20, 0}, {0.16, -0.18, 0},
      {0.46, -0.02, 0}, {0.20, 0.20, 0}, {-0.34, 0.18, 0},
    },
  },
  { color = "foreground", points = {{-0.38, 0.05, 0}, {-0.78, -0.18, 0}, {-0.62, 0.12, 0}} },
  { color = "foreground", points = {{-0.42, 0.08, 0}, {-0.72, 0.31, 0}, {-0.55, 0.02, 0}} },
  {
    color = "foreground",
    points = {
      {0.18, -0.12, 0}, {0.34, -0.38, 0}, {0.57, -0.42, 0},
      {0.68, -0.25, 0}, {0.50, -0.10, 0},
    },
  },
  { color = "yellow", points = {{0.62, -0.31, 0}, {0.92, -0.23, 0}, {0.62, -0.17, 0}} },
  {
    color = "accent",
    points = {
      {-0.20, -0.08, 0}, {-0.64, -0.86, -1}, {-0.26, -0.66, -0.72},
      {0.28, 0.02, 0},
    },
  },
  {
    color = "muted",
    points = {
      {-0.18, 0.04, 0}, {-0.54, 0.74, 1}, {-0.12, 0.52, 0.68},
      {0.24, 0.10, 0},
    },
  },
}

neoism.autocmd("AgentChanged", function(event)
  local payload = type(event.payload) == "table" and event.payload or nil
  local revision = type(payload) == "table" and payload.composerRevision or nil
  local length = type(payload) == "table" and payload.composerLength or 0
  if type(revision) == "number" and revision ~= last_revision then
    last_revision = revision
    if length > 0 then
      pcall(function()
        neoism.effect.emit({
          kind = "particles",
          seed = revision,
          durationSeconds = 1.65 + (revision % 6) * 0.11,
          angleMinDegrees = 190,
          angleMaxDegrees = 350,
          speedMin = 300,
          speedMax = 690,
          gravity = -42,
          wobble = 34,
          originSpread = 90,
          sizeMin = 14,
          sizeMax = 25,
          flapHz = 10 + (revision % 5),
          flapAmplitude = 0.52,
          polygons = bird_polygons,
        })
      end)
    end
  end
end)