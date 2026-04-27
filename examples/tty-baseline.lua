-- examples/tty-baseline.lua
--
-- This is the practical standalone tty example.
--
-- It keeps the same overall structure as the main baseline config, but adds:
-- - tty-specific backend selection
-- - tty quit / VT-switch settings
-- - repo-owned probe commands for manual testing
-- - a repo-owned layer-shell panel on startup

--------------------------------------------------------------------------------
-- Shared helpers and commands
--------------------------------------------------------------------------------

local common = include("lib/common.lua")
local shared_rules = include("rules.lua")
local commands = common.commands

local launcher_command = commands.launcher
local wayland_probe_command = "./scripts/example-launch.sh wayland-probe"
local panel_probe_command = "./scripts/example-launch.sh layer-panel"
local x11_test_command = "./scripts/example-launch.sh x11-probe-or-test"

--------------------------------------------------------------------------------
-- Small local hook helpers
--------------------------------------------------------------------------------

local resolve_focus = common.make_resolve_focus({ drag_modifier = "super" })

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
  backend = "udev",

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

  tty = {
    quit_keyspec = "Ctrl+Alt+Backspace",
    vt_switch_modifiers = { "Ctrl", "Alt" },
    output_layout = "horizontal",

    -- Touchpad-friendly defaults for the practical tty profile.
    tap_to_click = true,
    natural_scroll = false,
  },

  rules = shared_rules,
})

--------------------------------------------------------------------------------
-- Autostart and bindings
--------------------------------------------------------------------------------

-- Start one repo-owned layer-shell panel so the tty example has a predictable
-- panel/bar story instead of depending on whatever host desktop the machine has.
evil.autostart(panel_probe_command)

-- TTY test interactions in this config:
-- - Super+left click on a window: start interactive move
-- - Super+right click on a window: start interactive resize
-- - Super+D: launcher when available
-- - Super+Y: repo-owned Wayland probe window
-- - Super+X: repo-owned X11 probe window or fallback X11 app
-- - Super+Shift+S: screenshot helper (nested backend today)

evil.bind("Super+Return", "spawn", { command = commands.terminal })
evil.bind("Super+T", "spawn", { command = commands.terminal })
evil.bind("Super+W", "spawn", { command = commands.browser })
evil.bind("Super+E", "spawn", { command = commands.file_manager })
evil.bind("Super+D", "spawn", { command = launcher_command })
evil.bind("Super+Y", "spawn", { command = wayland_probe_command })
evil.bind("Super+X", "spawn", { command = x11_test_command })
evil.bind("Super+Shift+S", "spawn", { command = commands.screenshot })
evil.bind("Super+Q", "close_window")

evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

--------------------------------------------------------------------------------
-- Hook assignments
--------------------------------------------------------------------------------

evil.on.resolve_focus = resolve_focus
evil.on.move_update = move_window_with_pointer_delta
evil.on.resize_update = resize_window_with_pointer_delta
evil.on.draw_background = common.draw_background
evil.on.draw_window_overlay = common.draw_focus_border
