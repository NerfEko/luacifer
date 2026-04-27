-- examples/nested-debug.lua
--
-- This is the same general idea as examples/config.lua, but it changes the drag
-- modifier from Super to Alt.
--
-- Why?
-- Some host compositors steal Super+mouse before the nested Luacifer window ever
-- sees it. Using Alt here makes nested debugging easier under those hosts.

--------------------------------------------------------------------------------
-- Shared helpers
--------------------------------------------------------------------------------

local common = include("lib/common.lua")
local shared_rules = include("rules.lua")
local commands = common.commands

--------------------------------------------------------------------------------
-- Small local hook helpers
--------------------------------------------------------------------------------

-- Use Alt instead of Super for interactive move/resize in this config.
local resolve_focus = common.make_resolve_focus({ drag_modifier = "alt" })

local function move_window_with_pointer_delta(ctx)
  evil.window.move(ctx.window.id, ctx.window.x + ctx.dx, ctx.window.y + ctx.dy)
end

local function resize_window_with_pointer_delta(ctx)
  local bounds = common.resize_bounds(ctx.window, ctx.dx, ctx.dy, ctx.edges)
  evil.window.set_bounds(ctx.window.id, bounds.x, bounds.y, bounds.w, bounds.h)
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
  draw = {
    stack = { "background", "windows", "window_overlay", "popups", "overlay", "cursor" },
    clear_color = { 0.08, 0.05, 0.12, 1.0 },
  },
  window = {
    use_client_default_size = true,
    remember_sizes_by_app_id = true,
    hide_client_decorations = true,
  },
  placement = {
    default_size = { w = 900, h = 600 },
    padding = 32,
    cascade_step = { x = 32, y = 24 },
  },
  rules = shared_rules,
})

--------------------------------------------------------------------------------
-- Autostart and bindings
--------------------------------------------------------------------------------

evil.autostart(commands.terminal)

-- Nested-debug interactions:
-- - Alt+left click on a window: start interactive move
-- - Alt+right click on a window: start interactive resize
-- - Alt+D: app launcher
-- - Alt+X: simple X11 test path
-- - Alt+Shift+S: screenshot helper

evil.bind("Alt+Return", "spawn", { command = commands.terminal })
evil.bind("Alt+W", "spawn", { command = commands.browser })
evil.bind("Alt+E", "spawn", { command = commands.file_manager })
evil.bind("Alt+D", "spawn", { command = commands.launcher })
evil.bind("Alt+X", "spawn", { command = commands.x11_test })
evil.bind("Alt+Shift+S", "spawn", { command = commands.screenshot })
evil.bind("Alt+Q", "close_window")

evil.bind("Alt+H", "pan_left", { amount = 32 })
evil.bind("Alt+L", "pan_right", { amount = 32 })
evil.bind("Alt+J", "pan_down", { amount = 32 })
evil.bind("Alt+K", "pan_up", { amount = 32 })
evil.bind("Alt+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Alt+Minus", "zoom_out", { amount = 0.87 })

--------------------------------------------------------------------------------
-- Hook assignments
--------------------------------------------------------------------------------

evil.on.resolve_focus = resolve_focus
evil.on.move_update = move_window_with_pointer_delta
evil.on.resize_update = resize_window_with_pointer_delta
evil.on.draw_background = common.draw_background
evil.on.draw_window_overlay = common.draw_focus_border
