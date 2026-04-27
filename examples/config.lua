-- examples/config.lua
--
-- This is the main baseline example.
--
-- It shows the overall config shape the project expects most users to start
-- from:
--   1. load shared helpers
--   2. define small local hook helpers
--   3. call evil.config({...})
--   4. add autostart + bindings
--   5. assign hooks at the end
--
-- If you only want one example to copy and edit, start here.

--------------------------------------------------------------------------------
-- Shared helpers
--------------------------------------------------------------------------------

local common = include("lib/common.lua")
local shared_rules = include("rules.lua")
local commands = common.commands

--------------------------------------------------------------------------------
-- Small local hook helpers
--------------------------------------------------------------------------------

-- This focus policy comes from examples/lib/common.lua.
-- It gives us a simple, readable default:
-- - focus new windows when they map
-- - focus a clicked window
-- - clear focus when clicking empty space
-- - try to focus another window when one disappears
-- - allow Super+mouse drag/resize
local resolve_focus = common.make_resolve_focus()

local function move_window_with_pointer_delta(ctx)
  evil.window.move(ctx.window.id, ctx.window.x + ctx.dx, ctx.window.y + ctx.dy)
end

local function resize_window_with_pointer_delta(ctx)
  local bounds = common.resize_bounds(ctx.window, ctx.dx, ctx.dy, ctx.edges)
  evil.window.set_bounds(ctx.window.id, bounds.x, bounds.y, bounds.w, bounds.h)
end

--------------------------------------------------------------------------------
-- Main config table
--------------------------------------------------------------------------------

evil.config({
  backend = "winit",

  -- Canvas settings control how far you can zoom and how much the built-in
  -- pan/zoom actions move the camera each time.
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },

  -- Draw config controls compositor-side drawing layers.
  draw = {
    -- Layers are listed from bottom to top.
    -- Focused window outlines are drawn above normal windows but below popups.
    stack = { "background", "windows", "window_overlay", "popups", "overlay", "cursor" },
    clear_color = { 0.08, 0.05, 0.12, 1.0 },
  },

  -- Window config controls a few baseline Rust-side window facts.
  window = {
    use_client_default_size = true,
    remember_sizes_by_app_id = true,
    hide_client_decorations = true,
  },

  -- Placement is the fallback Rust placement path used when no Lua placement
  -- hook overrides it.
  placement = {
    default_size = { w = 900, h = 600 },
    padding = 32,
    cascade_step = { x = 32, y = 24 },
  },

  -- Rules are loaded from examples/rules.lua so this file stays focused on the
  -- overall structure.
  rules = shared_rules,
})

--------------------------------------------------------------------------------
-- Autostart and key bindings
--------------------------------------------------------------------------------

-- Autostart/spawn commands run through a shell on purpose, so simple quoting,
-- env expansion, and pipelines are allowed in trusted user config.
evil.autostart(commands.terminal)

-- App / utility binds
evil.bind("Super+Return", "spawn", { command = commands.terminal })
evil.bind("Super+Q", "close_window")
evil.bind("Super+D", "spawn", { command = commands.launcher })
evil.bind("Super+X", "spawn", { command = commands.x11_test })
evil.bind("Super+Shift+S", "spawn", { command = commands.screenshot })

-- Canvas navigation binds
evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

--------------------------------------------------------------------------------
-- Hook assignments
--------------------------------------------------------------------------------

-- Keep focus behavior simple and explicit.
evil.on.resolve_focus = resolve_focus

-- These movement hooks use imperative runtime commands directly.
-- That is the default example style in this repo because it is easier to read
-- and modify.
evil.on.move_update = move_window_with_pointer_delta
evil.on.resize_update = resize_window_with_pointer_delta

-- Compositor-side drawing hooks.
evil.on.draw_background = common.draw_background
evil.on.draw_window_overlay = common.draw_focus_border
