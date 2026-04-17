-- Public example config for the GitHub release mirror.
--
-- This is meant to be a real, readable baseline that shows the intended
-- direction of the Lua policy layer: keybindings, placement, focus behavior,
-- movement, resize handling, and compositor-side visuals in one file.
--
-- It is still example/prototype material, not a promise of long-term API stability.

local terminal_cmd = [[sh -lc 'if command -v foot >/dev/null 2>&1; then exec foot; elif command -v kitty >/dev/null 2>&1; then exec kitty; elif command -v alacritty >/dev/null 2>&1; then exec alacritty; elif command -v wezterm >/dev/null 2>&1; then exec wezterm; else exec xterm; fi']]
local browser_cmd = [[sh -lc 'if command -v firefox >/dev/null 2>&1; then exec firefox; elif command -v chromium >/dev/null 2>&1; then exec chromium; elif command -v google-chrome-stable >/dev/null 2>&1; then exec google-chrome-stable; else exec xdg-open https://example.com; fi']]
local launcher_cmd = [[sh -lc 'if command -v fuzzel >/dev/null 2>&1; then exec fuzzel; elif command -v bemenu-run >/dev/null 2>&1; then exec bemenu-run; elif command -v rofi >/dev/null 2>&1; then exec rofi -show drun; else exec xterm; fi']]

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

local function primary_output()
  return evil.output.primary()
end

local function cascade_bounds(ctx)
  local output = primary_output()
  local visible = output and output.visible_world or { x = 0, y = 0, w = 1600, h = 900 }
  local focused = focused_window(ctx)

  local w = math.min(960, math.max(520, visible.w * 0.55))
  local h = math.min(720, math.max(340, visible.h * 0.55))

  if focused then
    return {
      x = focused.x + 48,
      y = focused.y + 48,
      w = focused.w,
      h = focused.h,
    }
  end

  return {
    x = visible.x + ((visible.w - w) / 2),
    y = visible.y + ((visible.h - h) / 2),
    w = w,
    h = h,
  }
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

local function draw_dot_grid(ctx)
  local viewport = ctx.output.viewport
  local visible = viewport.visible_world
  local spacing = 128
  local major_every = 4
  local shapes = {}

  local start_x = math.floor(visible.x / spacing) - 1
  local end_x = math.ceil((visible.x + visible.w) / spacing) + 1
  local start_y = math.floor(visible.y / spacing) - 1
  local end_y = math.ceil((visible.y + visible.h) / spacing) + 1

  for gx = start_x, end_x do
    for gy = start_y, end_y do
      local world_x = gx * spacing
      local world_y = gy * spacing
      local screen_x = math.floor(((world_x - visible.x) * viewport.zoom) + 0.5)
      local screen_y = math.floor(((world_y - visible.y) * viewport.zoom) + 0.5)

      local major = gx % major_every == 0 and gy % major_every == 0
      local axis = gx == 0 or gy == 0
      local size = axis and 5 or (major and 4 or 2)
      local color = axis and { 0.55, 0.45, 0.85, 0.85 }
        or (major and { 0.38, 0.32, 0.62, 0.65 } or { 0.26, 0.24, 0.38, 0.5 })

      if screen_x + size >= 0 and screen_y + size >= 0
        and screen_x < viewport.screen_w and screen_y < viewport.screen_h then
        shapes[#shapes + 1] = evil.draw.rect({
          space = "screen",
          x = screen_x,
          y = screen_y,
          w = size,
          h = size,
          color = color,
        })
      end
    end
  end

  return shapes
end

evil.config({
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },
})

evil.bind("Super+Return", "spawn", { command = terminal_cmd })
evil.bind("Super+T", "spawn", { command = terminal_cmd })
evil.bind("Super+B", "spawn", { command = browser_cmd })
evil.bind("Super+Space", "spawn", { command = launcher_cmd })
evil.bind("Super+Q", "close_window")

evil.bind("Super+H", "pan_left", { amount = 48 })
evil.bind("Super+L", "pan_right", { amount = 48 })
evil.bind("Super+J", "pan_down", { amount = 48 })
evil.bind("Super+K", "pan_up", { amount = 48 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

evil.on.place_window = function(ctx)
  local bounds = cascade_bounds(ctx)
  return {
    kind = "set_bounds",
    id = ctx.window.id,
    x = bounds.x,
    y = bounds.y,
    w = bounds.w,
    h = bounds.h,
  }
end

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

evil.on.draw_background = draw_dot_grid

evil.on.draw_overlay = function(ctx)
  local window = focused_window(ctx)
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
      outer = 1,
      color = { 0.18, 0.15, 0.26, 0.95 },
    }),
    evil.draw.stroke_rect({
      space = "world",
      x = window.x,
      y = window.y,
      w = window.w,
      h = window.h,
      width = 3,
      color = { 0.74, 0.58, 0.98, 0.98 },
    }),
  }
end
