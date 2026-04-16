-- Public example config for the GitHub release mirror.
-- This file is intentionally small and self-contained: it exists to show
-- what the Lua policy layer looks like, not to promise a stable API yet.

local function first_focusable_window_from_end(windows)
  for index = #windows, 1, -1 do
    local window = windows[index]
    if not window.exclude_from_focus then
      return window
    end
  end
end

local function focused_window(ctx)
  if not ctx.focused_window_id then
    return nil
  end

  for _, window in ipairs(ctx.state.windows) do
    if window.id == ctx.focused_window_id then
      return window
    end
  end
end

local function resize_bounds(window, dx, dy, edges)
  local left = window.x
  local top = window.y
  local right = window.x + window.w
  local bottom = window.y + window.h

  if edges.left then left = left + dx end
  if edges.right then right = right + dx end
  if edges.top then top = top + dy end
  if edges.bottom then bottom = bottom + dy end

  local min_w = 160
  local min_h = 100

  if right - left < min_w then
    if edges.left and not edges.right then
      left = right - min_w
    else
      right = left + min_w
    end
  end

  if bottom - top < min_h then
    if edges.top and not edges.bottom then
      top = bottom - min_h
    else
      bottom = top + min_h
    end
  end

  return {
    x = left,
    y = top,
    w = right - left,
    h = bottom - top,
  }
end

evil.config({
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },
})

evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

evil.on.resolve_focus = function(ctx)
  if ctx.reason == "window_mapped" and ctx.window and not ctx.window.exclude_from_focus then
    return { kind = "focus_window", id = ctx.window.id }
  end

  if ctx.reason == "pointer_button" and ctx.pressed then
    if ctx.window and not ctx.window.exclude_from_focus then
      return { kind = "focus_window", id = ctx.window.id }
    end
    return { kind = "clear_focus" }
  end

  if ctx.reason == "window_unmapped" then
    local next_window = first_focusable_window_from_end(ctx.state.windows)
    if next_window then
      return { kind = "focus_window", id = next_window.id }
    end
    return { kind = "clear_focus" }
  end
end

evil.on.move_update = function(ctx)
  return {
    kind = "move_window",
    id = ctx.window.id,
    x = ctx.window.x + ctx.dx,
    y = ctx.window.y + ctx.dy,
  }
end

evil.on.resize_update = function(ctx)
  local bounds = resize_bounds(ctx.window, ctx.dx, ctx.dy, ctx.edges)
  return {
    kind = "set_bounds",
    id = ctx.window.id,
    x = bounds.x,
    y = bounds.y,
    w = bounds.w,
    h = bounds.h,
  }
end

evil.on.draw_overlay = function(ctx)
  local window = focused_window(ctx)
  if not window then
    return nil
  end

  return evil.draw.stroke_rect({
    space = "world",
    x = window.x,
    y = window.y,
    w = window.w,
    h = window.h,
    width = 2,
    color = { 0.74, 0.58, 0.98, 0.98 },
  })
end
