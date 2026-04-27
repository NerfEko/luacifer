-- Public example config for the GitHub release mirror.
--
-- This is meant to be a practical, self-contained baseline for the current
-- project direction: a small Rust compositor core with Lua-defined policy.
--
-- It targets the standalone `udev`/tty backend so the public baseline reflects
-- the current project direction more closely than a nested-in-a-window setup.
--
-- It is still example/prototype material, not a promise of long-term API
-- stability.

local BTN_LEFT = 272
local BTN_RIGHT = 273

local terminal_cmd = [[sh -lc 'if [ -n "${TERMINAL:-}" ] && command -v "${TERMINAL%% *}" >/dev/null 2>&1; then exec ${TERMINAL}; elif command -v kitty >/dev/null 2>&1; then exec kitty -1; elif command -v foot >/dev/null 2>&1; then exec foot; elif command -v alacritty >/dev/null 2>&1; then exec alacritty; elif command -v wezterm >/dev/null 2>&1; then exec wezterm; elif command -v konsole >/dev/null 2>&1; then exec konsole; elif command -v kgx >/dev/null 2>&1; then exec kgx; elif command -v uxterm >/dev/null 2>&1; then exec uxterm; else exec xterm; fi']]
local browser_cmd = [[sh -lc 'if command -v firefox >/dev/null 2>&1; then exec firefox; elif command -v chromium >/dev/null 2>&1; then exec chromium; elif command -v google-chrome-stable >/dev/null 2>&1; then exec google-chrome-stable; elif command -v brave >/dev/null 2>&1; then exec brave; elif command -v microsoft-edge-stable >/dev/null 2>&1; then exec microsoft-edge-stable; elif command -v opera >/dev/null 2>&1; then exec opera; elif command -v librewolf >/dev/null 2>&1; then exec librewolf; elif command -v flatpak >/dev/null 2>&1; then exec flatpak run app.zen_browser.zen; else exec xdg-open https://example.com; fi']]
local launcher_cmd = [[sh -lc 'if command -v rofi >/dev/null 2>&1; then exec rofi -show drun; elif command -v wofi >/dev/null 2>&1; then exec wofi --show drun; elif command -v fuzzel >/dev/null 2>&1; then exec fuzzel; else exec sh -lc "${TERMINAL:-xterm}"; fi']]
local file_manager_cmd = [[sh -lc 'if command -v nautilus >/dev/null 2>&1; then exec nautilus; elif command -v dolphin >/dev/null 2>&1; then exec dolphin; elif command -v nemo >/dev/null 2>&1; then exec nemo; elif command -v thunar >/dev/null 2>&1; then exec thunar; elif command -v yazi >/dev/null 2>&1; then exec sh -lc "${TERMINAL:-xterm} -e yazi"; else exec sh -lc "${TERMINAL:-xterm}"; fi']]
local wayland_probe_cmd = [[sh -lc 'bin="${LUACIFER_BIN:-luacifer}"; probe="$(dirname "$bin")/luacifer-probe-client"; if [ ! -x "$probe" ]; then printf "luacifer probe client not found: %s\n" "$probe" >&2; exit 1; fi; exec "$probe" xdg-window --title "luacifer probe wayland"']]
local panel_probe_cmd = [[sh -lc 'bin="${LUACIFER_BIN:-luacifer}"; probe="$(dirname "$bin")/luacifer-probe-client"; if [ ! -x "$probe" ]; then printf "luacifer probe client not found: %s\n" "$probe" >&2; exit 1; fi; exec "$probe" layer-panel --namespace luacifer-public-baseline-panel']]
local x11_test_cmd = [[sh -lc 'bin="${LUACIFER_BIN:-luacifer}"; probe="$(dirname "$bin")/luacifer-probe-client"; if [ -x "$probe" ]; then exec "$probe" x11-window --title "luacifer probe x11"; elif command -v xterm >/dev/null 2>&1; then exec xterm; elif command -v xeyes >/dev/null 2>&1; then exec xeyes; elif command -v xclock >/dev/null 2>&1; then exec xclock; elif command -v xcalc >/dev/null 2>&1; then exec xcalc; else printf "No simple X11 test app found (tried: luacifer-probe-client xterm xeyes xclock xcalc)\n" >&2; exit 1; fi']]
local screenshot_cmd = [[sh -lc 'bin="${LUACIFER_BIN:-luacifer}"; sock="${LUACIFER_IPC_SOCKET:-}"; if [ -z "$sock" ]; then printf "LUACIFER_IPC_SOCKET is not set\n" >&2; exit 1; fi; dir="${XDG_PICTURES_DIR:-$HOME/Pictures}/luacifer"; mkdir -p "$dir"; out="$dir/$(date +%Y-%m-%d_%H-%M-%S).ppm"; "$bin" --ipc-socket "$sock" --ipc-command screenshot --ipc-arg "$out" && printf "saved screenshot to %s\n" "$out"']]

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
  },
  window = {
    use_client_default_size = true,
    remember_sizes_by_app_id = true,
    hide_client_decorations = true,
  },
})

evil.autostart(panel_probe_cmd)

-- TTY baseline interactions:
-- - Super+left click on a window: start interactive move
-- - Super+right click on a window: start interactive resize from the clicked quadrant/corner
-- - Super+D: open the launcher when rofi/wofi/fuzzel is available
-- - Super+Y: launch the repo-owned Wayland probe window
-- - Super+X: launch the repo-owned X11 probe window when available, otherwise a simple X11 fallback
-- - Super+Shift+S: save a screenshot through IPC when supported by the running backend

evil.bind("Super+Return", "spawn", { command = terminal_cmd })
evil.bind("Super+T", "spawn", { command = terminal_cmd })
evil.bind("Super+W", "spawn", { command = browser_cmd })
evil.bind("Super+E", "spawn", { command = file_manager_cmd })
evil.bind("Super+D", "spawn", { command = launcher_cmd })
evil.bind("Super+Y", "spawn", { command = wayland_probe_cmd })
evil.bind("Super+X", "spawn", { command = x11_test_cmd })
evil.bind("Super+Shift+S", "spawn", { command = screenshot_cmd })
evil.bind("Super+Q", "close_window")

evil.bind("Super+H", "pan_left", { amount = 32 })
evil.bind("Super+L", "pan_right", { amount = 32 })
evil.bind("Super+J", "pan_down", { amount = 32 })
evil.bind("Super+K", "pan_up", { amount = 32 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })
evil.bind("Super+Minus", "zoom_out", { amount = 0.87 })

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

local function preferred_output()
  return evil.output.at_pointer() or evil.output.primary()
end

local function clamp_size(value, min_value, max_value)
  return math.max(min_value, math.min(max_value, value))
end

local function place_window_bounds(ctx)
  local output = preferred_output()
  local visible = output and output.visible_world or { x = 0, y = 0, w = 1600, h = 900 }
  local focused = focused_window(ctx)

  local max_w = math.max(320, visible.w - 96)
  local max_h = math.max(220, visible.h - 96)
  local width = clamp_size(ctx.window.w or (visible.w * 0.62), 520, max_w)
  local height = clamp_size(ctx.window.h or (visible.h * 0.62), 320, max_h)

  if focused then
    return {
      x = focused.x + 48,
      y = focused.y + 48,
      w = clamp_size(focused.w, 520, max_w),
      h = clamp_size(focused.h, 320, max_h),
    }
  end

  return {
    x = visible.x + ((visible.w - width) / 2),
    y = visible.y + ((visible.h - height) / 2),
    w = width,
    h = height,
  }
end

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

local function resize_bounds(window, dx, dy, edges)
  local left = window.x
  local top = window.y
  local right = window.x + window.w
  local bottom = window.y + window.h

  if edges.left then
    left = left + dx
  end
  if edges.right then
    right = right + dx
  end
  if edges.top then
    top = top + dy
  end
  if edges.bottom then
    bottom = bottom + dy
  end

  local min_w = 120
  local min_h = 80

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

local function adaptive_grid_spacing(viewport)
  local spacing = 128
  local min_screen_spacing = 28

  while (spacing * viewport.zoom) < min_screen_spacing do
    spacing = spacing * 2
  end

  return spacing
end

local function draw_dot_grid(ctx)
  local viewport = ctx.output.viewport
  local visible = viewport.visible_world
  local spacing = adaptive_grid_spacing(viewport)
  local major_every = spacing == 128 and 4 or 1
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

local function draw_focus_border(ctx)
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

local function resolve_focus(ctx)
  if ctx.reason == "window_mapped" and ctx.window and not ctx.window.exclude_from_focus then
    return { kind = "focus_window", id = ctx.window.id }
  end

  if ctx.reason == "pointer_button" and ctx.pressed then
    local mods = ctx.modifiers or {}

    if ctx.window and not ctx.window.exclude_from_focus then
      if mods.super and ctx.button == BTN_LEFT then
        return {
          actions = {
            { kind = "focus_window", id = ctx.window.id },
            { kind = "begin_move", id = ctx.window.id },
          },
        }
      end

      if mods.super and ctx.button == BTN_RIGHT then
        return {
          actions = {
            { kind = "focus_window", id = ctx.window.id },
            {
              kind = "begin_resize",
              id = ctx.window.id,
              edges = resize_edges_for_pointer(ctx.window, ctx.pointer),
            },
          },
        }
      end

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

local function place_window(ctx)
  local bounds = place_window_bounds(ctx)
  return {
    kind = "set_bounds",
    id = ctx.window.id,
    x = bounds.x,
    y = bounds.y,
    w = bounds.w,
    h = bounds.h,
  }
end

local function move_update(ctx)
  return {
    kind = "move_window",
    id = ctx.window.id,
    x = ctx.window.x + ctx.dx,
    y = ctx.window.y + ctx.dy,
  }
end

local function resize_update(ctx)
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

evil.on.resolve_focus = resolve_focus
evil.on.place_window = place_window
evil.on.move_update = move_update
evil.on.resize_update = resize_update
evil.on.draw_background = draw_dot_grid
evil.on.draw_window_overlay = draw_focus_border
