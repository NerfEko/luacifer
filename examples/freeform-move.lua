-- examples/freeform-move.lua
--
-- This example keeps the same overall layout as examples/config.lua but uses a
-- very small, direct movement policy.
--
-- It is good if you want to see the minimum amount of Lua needed for:
-- - basic focus handling
-- - freeform move
-- - freeform resize

--------------------------------------------------------------------------------
-- Shared data
--------------------------------------------------------------------------------

local shared_rules = include("rules.lua")

--------------------------------------------------------------------------------
-- Focus helpers
--------------------------------------------------------------------------------

local function first_focusable_window_from_end(windows)
  for index = #windows, 1, -1 do
    local window = windows[index]
    if not window.exclude_from_focus then
      return window
    end
  end
end

local function resolve_focus(ctx)
  if ctx.reason == "window_mapped" and ctx.window and not ctx.window.exclude_from_focus then
    evil.window.focus(ctx.window.id)
    return
  end

  if ctx.reason == "pointer_button" and ctx.pressed then
    if ctx.window and not ctx.window.exclude_from_focus then
      evil.window.focus(ctx.window.id)
    else
      evil.window.clear_focus()
    end
    return
  end

  if ctx.reason == "window_unmapped" then
    local next_window = first_focusable_window_from_end(ctx.state.windows)
    if next_window then
      evil.window.focus(next_window.id)
    else
      evil.window.clear_focus()
    end
  end
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
-- Autostart and bindings
--------------------------------------------------------------------------------

evil.autostart("foot")

evil.bind("Super+Return", "spawn", { command = "foot" })
evil.bind("Super+Q", "close_window")
evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

--------------------------------------------------------------------------------
-- Hooks
--------------------------------------------------------------------------------

evil.on.resolve_focus = resolve_focus

-- Imperative style:
-- move the window directly inside the hook.
evil.on.move_update = function(ctx)
  evil.window.move(ctx.window.id, ctx.window.x + ctx.dx, ctx.window.y + ctx.dy)
end

-- Imperative style again:
-- resize the window directly inside the hook.
evil.on.resize_update = function(ctx)
  evil.window.resize(ctx.window.id, ctx.window.w + ctx.dx, ctx.window.h + ctx.dy)
end
