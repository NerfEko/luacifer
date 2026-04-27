-- examples/lib/common.lua
--
-- This file holds the shared helper functions used by several example configs.
-- The goal is not to be clever. The goal is to show small, readable pieces that
-- you can copy into your own config and change.

local M = {}

-- Wayland pointer button codes used by the compositor.
local BTN_LEFT = 272
local BTN_RIGHT = 273

--------------------------------------------------------------------------------
-- Window lookup helpers
--------------------------------------------------------------------------------

-- Return the most recently listed focusable window.
--
-- We walk backward because the newest / most recently relevant windows tend to
-- be near the end of the snapshot list in these simple examples.
function M.first_focusable_window_from_end(windows)
  for index = #windows, 1, -1 do
    local window = windows[index]
    if not window.exclude_from_focus then
      return window
    end
  end
end

-- Return the focused window object from a hook context.
function M.focused_window(ctx)
  if not ctx.focused_window_id then
    return nil
  end

  for _, window in ipairs(ctx.state.windows) do
    if window.id == ctx.focused_window_id then
      return window
    end
  end
end

--------------------------------------------------------------------------------
-- Drawing helpers
--------------------------------------------------------------------------------

-- Choose a grid spacing that stays readable at different zoom levels.
function M.adaptive_grid_spacing(viewport)
  local spacing = 128
  local minimum_screen_spacing = 28

  while (spacing * viewport.zoom) < minimum_screen_spacing do
    spacing = spacing * 2
  end

  return spacing
end

-- Return the screen-space rectangle used for one grid dot.
local function grid_dot_shape(screen_x, screen_y, size, color)
  return evil.draw.rect({
    space = "screen",
    x = screen_x,
    y = screen_y,
    w = size,
    h = size,
    color = color,
  })
end

-- Return the size/color for one grid point.
local function grid_dot_style(grid_x, grid_y, major_every)
  local is_major = grid_x % major_every == 0 and grid_y % major_every == 0
  local is_axis = grid_x == 0 or grid_y == 0

  if is_axis then
    return 5, { 0.55, 0.45, 0.85, 0.85 }
  end

  if is_major then
    return 4, { 0.38, 0.32, 0.62, 0.65 }
  end

  return 2, { 0.26, 0.24, 0.38, 0.5 }
end

-- Draw a simple dot grid in screen space.
function M.draw_dot_grid(ctx)
  local viewport = ctx.output.viewport
  local visible = viewport.visible_world
  local spacing = M.adaptive_grid_spacing(viewport)
  local major_every = spacing == 128 and 4 or 1
  local shapes = {}

  local start_x = math.floor(visible.x / spacing) - 1
  local end_x = math.ceil((visible.x + visible.w) / spacing) + 1
  local start_y = math.floor(visible.y / spacing) - 1
  local end_y = math.ceil((visible.y + visible.h) / spacing) + 1

  for grid_x = start_x, end_x do
    for grid_y = start_y, end_y do
      local world_x = grid_x * spacing
      local world_y = grid_y * spacing
      local screen_x = math.floor(((world_x - visible.x) * viewport.zoom) + 0.5)
      local screen_y = math.floor(((world_y - visible.y) * viewport.zoom) + 0.5)

      local size, color = grid_dot_style(grid_x, grid_y, major_every)
      local dot_is_visible = screen_x + size >= 0
        and screen_y + size >= 0
        and screen_x < viewport.screen_w
        and screen_y < viewport.screen_h

      if dot_is_visible then
        shapes[#shapes + 1] = grid_dot_shape(screen_x, screen_y, size, color)
      end
    end
  end

  return shapes
end

-- Draw a two-layer outline around the focused window.
function M.draw_focus_border(ctx)
  local window = M.focused_window(ctx)
  if not window then
    return nil
  end

  return {
    evil.draw.stroke_rect({
      space = "world",
      x = window.x,
      y = window.y,
      w = window.w,
      h = window.h,
      width = 1,
      outer = 4,
      color = { 0.18, 0.15, 0.26, 0.95 },
    }),
    evil.draw.stroke_rect({
      space = "world",
      x = window.x,
      y = window.y,
      w = window.w,
      h = window.h,
      width = 2,
      outer = 2,
      color = { 0.74, 0.58, 0.98, 0.98 },
    }),
  }
end

-- The examples use the dot grid as their background.
function M.draw_background(ctx)
  return M.draw_dot_grid(ctx)
end

--------------------------------------------------------------------------------
-- Resize helpers
--------------------------------------------------------------------------------

local function apply_horizontal_resize(left, right, dx, edges)
  if edges.left then
    left = left + dx
  end
  if edges.right then
    right = right + dx
  end
  return left, right
end

local function apply_vertical_resize(top, bottom, dy, edges)
  if edges.top then
    top = top + dy
  end
  if edges.bottom then
    bottom = bottom + dy
  end
  return top, bottom
end

local function clamp_resize_width(left, right, edges, minimum_width)
  if right - left < minimum_width then
    if edges.left and not edges.right then
      left = right - minimum_width
    else
      right = left + minimum_width
    end
  end
  return left, right
end

local function clamp_resize_height(top, bottom, edges, minimum_height)
  if bottom - top < minimum_height then
    if edges.top and not edges.bottom then
      top = bottom - minimum_height
    else
      bottom = top + minimum_height
    end
  end
  return top, bottom
end

-- Return new bounds for a resize update.
function M.resize_bounds(window, dx, dy, edges)
  local left = window.x
  local top = window.y
  local right = window.x + window.w
  local bottom = window.y + window.h

  left, right = apply_horizontal_resize(left, right, dx, edges)
  top, bottom = apply_vertical_resize(top, bottom, dy, edges)

  left, right = clamp_resize_width(left, right, edges, 120)
  top, bottom = clamp_resize_height(top, bottom, edges, 80)

  return {
    x = left,
    y = top,
    w = right - left,
    h = bottom - top,
  }
end

--------------------------------------------------------------------------------
-- Focus / interactive helpers
--------------------------------------------------------------------------------

local function resize_edges_for_pointer(window, pointer)
  local pointer_x = pointer and pointer.x or (window.x + window.w / 2)
  local pointer_y = pointer and pointer.y or (window.y + window.h / 2)
  local center_x = window.x + window.w / 2
  local center_y = window.y + window.h / 2

  return {
    left = pointer_x < center_x,
    right = pointer_x >= center_x,
    top = pointer_y < center_y,
    bottom = pointer_y >= center_y,
  }
end

local function focus_window(window)
  if window then
    evil.window.focus(window.id)
  end
end

-- Build a simple resolve_focus hook.
--
-- Optional settings:
--   { drag_modifier = "super" }
--
-- When drag_modifier is set:
-- - modifier + left click starts interactive move
-- - modifier + right click starts interactive resize
function M.make_resolve_focus(options)
  local drag_modifier = options and options.drag_modifier or nil

  return function(ctx)
    if ctx.reason == "window_mapped" and ctx.window and not ctx.window.exclude_from_focus then
      focus_window(ctx.window)
      return
    end

    if ctx.reason == "pointer_button" and ctx.pressed then
      local modifiers = ctx.modifiers or {}

      if ctx.window and not ctx.window.exclude_from_focus then
        focus_window(ctx.window)

        if drag_modifier and modifiers[drag_modifier] and ctx.button == BTN_LEFT then
          evil.window.begin_move(ctx.window.id)
          return
        end

        if drag_modifier and modifiers[drag_modifier] and ctx.button == BTN_RIGHT then
          evil.window.begin_resize(ctx.window.id, resize_edges_for_pointer(ctx.window, ctx.pointer))
          return
        end

        return
      end

      evil.window.clear_focus()
      return
    end

    if ctx.reason == "window_unmapped" then
      local next_window = M.first_focusable_window_from_end(ctx.state.windows)
      if next_window then
        focus_window(next_window)
      else
        evil.window.clear_focus()
      end
    end
  end
end

--------------------------------------------------------------------------------
-- Example shell commands
--------------------------------------------------------------------------------

-- These commands run through the example launcher helper script.
-- Keeping them here makes the example configs shorter and easier to swap around.
M.commands = {
  terminal = "./scripts/example-launch.sh terminal",
  launcher = "./scripts/example-launch.sh launcher",
  x11_test = "./scripts/example-launch.sh x11-test",
  screenshot = "./scripts/example-launch.sh screenshot",
  browser = "./scripts/example-launch.sh browser",
  file_manager = "./scripts/example-launch.sh file-manager",
}

return M
