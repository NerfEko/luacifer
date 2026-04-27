-- examples/snap-grid.lua
--
-- This example shows one simple idea:
-- every move and resize snaps to a fixed grid.
--
-- If you want to experiment with spatial movement but still keep windows neatly
-- aligned, this is a good config to copy.

--------------------------------------------------------------------------------
-- Shared data
--------------------------------------------------------------------------------

local shared_rules = include("rules.lua")

--------------------------------------------------------------------------------
-- User-tunable settings
--------------------------------------------------------------------------------

local GRID_SIZE = 64

--------------------------------------------------------------------------------
-- Small helper functions
--------------------------------------------------------------------------------

local function snap_to_grid(value)
  return math.floor((value / GRID_SIZE) + 0.5) * GRID_SIZE
end

local function snapped_resize(value)
  return math.max(GRID_SIZE, snap_to_grid(value))
end

--------------------------------------------------------------------------------
-- Main config
--------------------------------------------------------------------------------

evil.config({
  backend = "winit",
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },
  rules = shared_rules,
})

--------------------------------------------------------------------------------
-- Bindings
--------------------------------------------------------------------------------

evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

--------------------------------------------------------------------------------
-- Hooks
--------------------------------------------------------------------------------

-- Imperative style:
-- move the window directly inside the hook.
evil.on.move_update = function(ctx)
  evil.window.move(
    ctx.window.id,
    snap_to_grid(ctx.window.x + ctx.dx),
    snap_to_grid(ctx.window.y + ctx.dy)
  )
end

-- Keep width/height snapped too, and never shrink below one grid cell.
evil.on.resize_update = function(ctx)
  evil.window.resize(
    ctx.window.id,
    snapped_resize(ctx.window.w + ctx.dx),
    snapped_resize(ctx.window.h + ctx.dy)
  )
end
