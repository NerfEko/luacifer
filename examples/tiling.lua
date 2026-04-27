-- examples/tiling.lua
--
-- This example tries to feel like a tiling window manager while still running on
-- Luacifer's shared canvas model.
--
-- The main idea is simple:
-- - pretend the canvas is 10 screen-sized "desktop pages" placed side by side
-- - keep zoom locked to 1.0 so one page always matches one screen
-- - tile windows inside the current page
-- - move the camera between pages instead of freely panning around
--
-- This example also shows two "creative Lua" tricks:
-- - floating mode is tracked in Lua tables
-- - fullscreen mode is tracked in Lua tables
--
-- They are not native runtime features yet, but they behave well enough for a
-- strict example profile.

--------------------------------------------------------------------------------
-- Shared helpers and user-tunable settings
--------------------------------------------------------------------------------

local common = include("lib/common.lua")
local commands = common.commands

local BTN_LEFT = 272
local BTN_RIGHT = 273

local PAGE_COUNT = 10
local OUTER_GAP = 18
local TILE_GAP = 10

--------------------------------------------------------------------------------
-- Lua-owned state for this config
--------------------------------------------------------------------------------

-- The page the camera is currently showing.
local current_page = 1

-- Which page each window belongs to.
local page_for_window = {}

-- Which windows are currently floating.
local floating_for_window = {}

-- Saved floating bounds for those windows.
local floating_bounds = {}

-- Which window (if any) is fullscreen on each page.
local fullscreen_for_page = {}

-- Per-page tiling trees. Each root is either:
-- - { kind = "leaf", window_id = ... }
-- - { kind = "split", axis = "vertical" | "horizontal", first = ..., second = ... }
local page_layout_roots = {}

--------------------------------------------------------------------------------
-- Basic page / window lookup helpers
--------------------------------------------------------------------------------

local function clamp_page(page)
  if page < 1 then
    return 1
  end
  if page > PAGE_COUNT then
    return PAGE_COUNT
  end
  return page
end

local function page_index_for_key(key)
  if key == "0" then
    return 10
  end

  local numeric = tonumber(key)
  if numeric and numeric >= 1 and numeric <= 9 then
    return numeric
  end

  return nil
end

local function screen_metrics(state)
  local output = state.outputs[1]
  if not output then
    return nil
  end

  return {
    viewport_x = output.viewport.world_x,
    viewport_y = output.viewport.world_y,
    screen_w = output.screen_bounds.w,
    screen_h = output.screen_bounds.h,
  }
end

local function page_origin_x(page, screen_w)
  return (page - 1) * screen_w
end

local function page_of_window(window_id)
  return page_for_window[window_id] or 1
end

local function window_for_id(state, id)
  for _, window in ipairs(state.windows) do
    if window.id == id then
      return window
    end
  end
end

--------------------------------------------------------------------------------
-- Layout helper functions
--------------------------------------------------------------------------------

local function page_rect(page, screen)
  return {
    x = page_origin_x(page, screen.screen_w),
    y = 0,
    w = screen.screen_w,
    h = screen.screen_h,
  }
end

local function page_inner_rect(page, screen)
  local rect = page_rect(page, screen)

  return {
    x = rect.x + OUTER_GAP,
    y = rect.y + OUTER_GAP,
    w = math.max(1, rect.w - (OUTER_GAP * 2)),
    h = math.max(1, rect.h - (OUTER_GAP * 2)),
  }
end

local function default_floating_bounds(page, screen)
  local width = math.max(1, math.floor(screen.screen_w * 0.7))
  local height = math.max(1, math.floor(screen.screen_h * 0.7))
  local x = page_origin_x(page, screen.screen_w) + math.floor((screen.screen_w - width) / 2)
  local y = math.floor((screen.screen_h - height) / 2)

  return {
    x = x,
    y = y,
    w = width,
    h = height,
  }
end

local function hidden_bounds(page, screen, slot)
  -- When a page is fullscreen, we move the other windows far outside the normal
  -- page strip instead of trying to unmap them.
  local hidden_x = (PAGE_COUNT * screen.screen_w) + 1024 + (slot * 32)

  return {
    x = hidden_x,
    y = slot * 32,
    w = 1,
    h = 1,
  }
end

local function set_window_bounds(id, bounds)
  evil.window.set_bounds(id, bounds.x, bounds.y, bounds.w, bounds.h)
end

--------------------------------------------------------------------------------
-- Window ordering helpers
--------------------------------------------------------------------------------

local function compare_windows_for_focus(a, b)
  if a.focused ~= b.focused then
    return a.focused
  end

  local a_focus = a.last_focused_at or -1
  local b_focus = b.last_focused_at or -1
  if a_focus ~= b_focus then
    return a_focus > b_focus
  end

  local a_map = a.mapped_at or 0
  local b_map = b.mapped_at or 0
  if a_map ~= b_map then
    return a_map < b_map
  end

  return a.id < b.id
end

local function sorted_focusable_windows_for_page(state, page)
  local windows = {}

  for _, window in ipairs(state.windows) do
    if not window.exclude_from_focus and page_of_window(window.id) == page then
      windows[#windows + 1] = window
    end
  end

  table.sort(windows, compare_windows_for_focus)
  return windows
end

local function tiled_windows_for_page(state, page)
  local tiled = {}
  local fullscreen_id = fullscreen_for_page[page]

  for _, window in ipairs(sorted_focusable_windows_for_page(state, page)) do
    local is_fullscreen_window = window.id == fullscreen_id
    local is_floating_window = floating_for_window[window.id]

    if not is_fullscreen_window and not is_floating_window then
      tiled[#tiled + 1] = window
    end
  end

  return tiled
end

local function page_focus_candidate(state, page)
  local fullscreen_id = fullscreen_for_page[page]
  if fullscreen_id then
    local fullscreen_window = window_for_id(state, fullscreen_id)
    if fullscreen_window and page_of_window(fullscreen_window.id) == page then
      return fullscreen_window
    end

    -- If the remembered fullscreen window is gone, clear the stale state.
    fullscreen_for_page[page] = nil
  end

  local windows = sorted_focusable_windows_for_page(state, page)
  return windows[1]
end

--------------------------------------------------------------------------------
-- Per-page layout builders
--------------------------------------------------------------------------------

local function leaf_node(window_id)
  return {
    kind = "leaf",
    window_id = window_id,
  }
end

local function split_axis_for_bounds(bounds)
  if bounds.w >= bounds.h then
    return "vertical"
  end

  return "horizontal"
end

local function split_bounds(bounds, axis)
  if axis == "vertical" then
    local available_w = math.max(1, bounds.w - TILE_GAP)
    local first_w = math.max(1, math.floor(available_w / 2))
    local second_w = math.max(1, bounds.w - first_w - TILE_GAP)

    return {
      x = bounds.x,
      y = bounds.y,
      w = first_w,
      h = bounds.h,
    }, {
      x = bounds.x + first_w + TILE_GAP,
      y = bounds.y,
      w = second_w,
      h = bounds.h,
    }
  end

  local available_h = math.max(1, bounds.h - TILE_GAP)
  local first_h = math.max(1, math.floor(available_h / 2))
  local second_h = math.max(1, bounds.h - first_h - TILE_GAP)

  return {
    x = bounds.x,
    y = bounds.y,
    w = bounds.w,
    h = first_h,
  }, {
    x = bounds.x,
    y = bounds.y + first_h + TILE_GAP,
    w = bounds.w,
    h = second_h,
  }
end

local function apply_layout_node(node, bounds)
  if not node then
    return
  end

  if node.kind == "leaf" then
    set_window_bounds(node.window_id, bounds)
    return
  end

  local first_bounds, second_bounds = split_bounds(bounds, node.axis)
  apply_layout_node(node.first, first_bounds)
  apply_layout_node(node.second, second_bounds)
end

local function replace_leaf_with_split(node, target_id, new_id, target_bounds)
  if not node then
    return false
  end

  if node.kind == "leaf" then
    if node.window_id ~= target_id then
      return false
    end

    node.kind = "split"
    node.axis = split_axis_for_bounds(target_bounds)
    node.first = leaf_node(target_id)
    node.second = leaf_node(new_id)
    node.window_id = nil
    return true
  end

  if replace_leaf_with_split(node.first, target_id, new_id, target_bounds) then
    return true
  end

  return replace_leaf_with_split(node.second, target_id, new_id, target_bounds)
end

local function remove_window_from_layout(node, window_id)
  if not node then
    return nil, false
  end

  if node.kind == "leaf" then
    if node.window_id == window_id then
      return nil, true
    end

    return node, false
  end

  local first, removed_first = remove_window_from_layout(node.first, window_id)
  local second, removed_second = remove_window_from_layout(node.second, window_id)

  if not removed_first and not removed_second then
    return node, false
  end

  node.first = first
  node.second = second

  if not node.first then
    return node.second, true
  end
  if not node.second then
    return node.first, true
  end

  return node, true
end

local function window_contains_point(window, point)
  if not point then
    return false
  end

  return point.x >= window.x
    and point.x < (window.x + window.w)
    and point.y >= window.y
    and point.y < (window.y + window.h)
end

local function insertion_target_window(state, page, new_window_id)
  local hovered = nil
  local focused = nil
  local fallback = nil

  for _, window in ipairs(state.windows) do
    if window.id ~= new_window_id
      and page_of_window(window.id) == page
      and not floating_for_window[window.id]
    then
      if window_contains_point(window, state.pointer) then
        hovered = window
      end
      if window.focused then
        focused = window
      end
      fallback = fallback or window
    end
  end

  return hovered or focused or fallback
end

local function insert_tiled_window_into_layout(state, page, window_id)
  local root = page_layout_roots[page]
  if not root then
    page_layout_roots[page] = leaf_node(window_id)
    return
  end

  local target = insertion_target_window(state, page, window_id)
  if not target then
    page_layout_roots[page] = {
      kind = "split",
      axis = "vertical",
      first = root,
      second = leaf_node(window_id),
    }
    return
  end

  if not replace_leaf_with_split(root, target.id, window_id, target) then
    page_layout_roots[page] = {
      kind = "split",
      axis = split_axis_for_bounds(target),
      first = root,
      second = leaf_node(window_id),
    }
  end
end

local function apply_fullscreen_layout(state, page, screen, fullscreen_window)
  local slot = 1
  local fullscreen_bounds = page_rect(page, screen)

  for _, window in ipairs(sorted_focusable_windows_for_page(state, page)) do
    if window.id == fullscreen_window.id then
      set_window_bounds(window.id, fullscreen_bounds)
    else
      set_window_bounds(window.id, hidden_bounds(page, screen, slot))
      slot = slot + 1
    end
  end
end

local function apply_tiled_layout(page, screen)
  local root = page_layout_roots[page]
  if not root then
    return
  end

  apply_layout_node(root, page_inner_rect(page, screen))
end

local function apply_floating_layout(state, page, screen)
  for _, window in ipairs(sorted_focusable_windows_for_page(state, page)) do
    if floating_for_window[window.id] then
      local bounds = floating_bounds[window.id] or default_floating_bounds(page, screen)
      floating_bounds[window.id] = bounds
      set_window_bounds(window.id, bounds)
    end
  end
end

--------------------------------------------------------------------------------
-- Main relayout function
--------------------------------------------------------------------------------

local function relayout(state)
  local screen = screen_metrics(state)
  if not screen then
    return false
  end

  for page = 1, PAGE_COUNT do
    local fullscreen_id = fullscreen_for_page[page]
    local fullscreen_window = fullscreen_id and window_for_id(state, fullscreen_id) or nil

    if fullscreen_window and page_of_window(fullscreen_window.id) == page then
      apply_fullscreen_layout(state, page, screen, fullscreen_window)
    else
      fullscreen_for_page[page] = nil
      apply_tiled_layout(page, screen)
      apply_floating_layout(state, page, screen)
    end
  end

  return true
end

--------------------------------------------------------------------------------
-- Camera helpers
--------------------------------------------------------------------------------

local function show_page(state, page)
  local screen = screen_metrics(state)
  if not screen then
    return false
  end

  local target_page = clamp_page(page)
  local target_x = page_origin_x(target_page, screen.screen_w)
  local dx = target_x - screen.viewport_x
  local dy = -screen.viewport_y

  evil.canvas.pan(dx, dy)

  local focus = page_focus_candidate(state, target_page)
  if focus then
    if state.focused_window_id ~= focus.id then
      evil.window.focus(focus.id)
    end
  elseif state.focused_window_id ~= nil then
    evil.window.clear_focus()
  end

  current_page = target_page
  return true
end

--------------------------------------------------------------------------------
-- Focus / page movement helpers
--------------------------------------------------------------------------------

local function cycle_focus_on_page(state, step)
  local windows = sorted_focusable_windows_for_page(state, current_page)
  if #windows == 0 then
    return false
  end

  local current_index = 1
  for index, window in ipairs(windows) do
    if window.focused then
      current_index = index
      break
    end
  end

  local next_index = current_index + step
  if next_index < 1 then
    next_index = #windows
  elseif next_index > #windows then
    next_index = 1
  end

  evil.window.focus(windows[next_index].id)
  return true
end

local function move_focused_window_to_page(state, page)
  local focused = evil.window.focused()
  if not focused then
    return false
  end

  local screen = screen_metrics(state)
  if not screen then
    return false
  end

  local old_page = page_of_window(focused.id)
  local target_page = clamp_page(page)
  if old_page == target_page then
    return false
  end

  if floating_for_window[focused.id] and floating_bounds[focused.id] then
    local bounds = floating_bounds[focused.id]
    bounds.x = bounds.x + ((target_page - old_page) * screen.screen_w)
    floating_bounds[focused.id] = bounds
  else
    page_layout_roots[old_page] = remove_window_from_layout(page_layout_roots[old_page], focused.id)
  end

  if fullscreen_for_page[old_page] == focused.id then
    fullscreen_for_page[old_page] = nil
  end

  page_for_window[focused.id] = target_page

  if not floating_for_window[focused.id] then
    insert_tiled_window_into_layout(state, target_page, focused.id)
  end

  relayout(state)
  show_page(state, target_page)
  return true
end

--------------------------------------------------------------------------------
-- Float / fullscreen helpers
--------------------------------------------------------------------------------

local function toggle_floating(state)
  local focused = evil.window.focused()
  if not focused then
    return false
  end

  local screen = screen_metrics(state)
  if not screen then
    return false
  end

  local page = page_of_window(focused.id)

  if floating_for_window[focused.id] then
    floating_for_window[focused.id] = nil
    floating_bounds[focused.id] = nil

    if fullscreen_for_page[page] == focused.id then
      fullscreen_for_page[page] = nil
    end

    insert_tiled_window_into_layout(state, page, focused.id)
    relayout(state)
    return true
  end

  floating_for_window[focused.id] = true
  page_layout_roots[page] = remove_window_from_layout(page_layout_roots[page], focused.id)

  local current_window = window_for_id(state, focused.id)
  local page_bounds = page_inner_rect(page, screen)
  local bounds = default_floating_bounds(page, screen)

  if current_window then
    local already_full_page = current_window.x == page_bounds.x
      and current_window.y == page_bounds.y
      and current_window.w == page_bounds.w
      and current_window.h == page_bounds.h

    if not already_full_page then
      bounds = {
        x = current_window.x,
        y = current_window.y,
        w = current_window.w,
        h = current_window.h,
      }
    end
  end

  floating_bounds[focused.id] = bounds

  relayout(state)
  evil.window.focus(focused.id)
  return true
end

local function toggle_fullscreen(state)
  local focused = evil.window.focused()
  if not focused then
    return false
  end

  local page = page_of_window(focused.id)

  if fullscreen_for_page[page] == focused.id then
    fullscreen_for_page[page] = nil
  else
    fullscreen_for_page[page] = focused.id
  end

  relayout(state)
  evil.window.focus(focused.id)
  return true
end

--------------------------------------------------------------------------------
-- Pointer / drag helpers for floating windows
--------------------------------------------------------------------------------

local function resize_edges_for_floating_window(ctx)
  local pointer_x = ctx.pointer and ctx.pointer.x or (ctx.window.x + (ctx.window.w / 2))
  local pointer_y = ctx.pointer and ctx.pointer.y or (ctx.window.y + (ctx.window.h / 2))
  local center_x = ctx.window.x + (ctx.window.w / 2)
  local center_y = ctx.window.y + (ctx.window.h / 2)

  return {
    left = pointer_x < center_x,
    right = pointer_x >= center_x,
    top = pointer_y < center_y,
    bottom = pointer_y >= center_y,
  }
end

--------------------------------------------------------------------------------
-- Focus policy
--------------------------------------------------------------------------------

local function tiling_resolve_focus(ctx)
  if ctx.reason == "pointer_motion" and ctx.window and not ctx.window.exclude_from_focus then
    local target_page = page_of_window(ctx.window.id)
    show_page(ctx.state, target_page)

    if ctx.state.focused_window_id ~= ctx.window.id then
      evil.window.focus(ctx.window.id)
    end
    return
  end

  if ctx.reason == "pointer_button" and ctx.pressed then
    if ctx.window and not ctx.window.exclude_from_focus then
      local target_page = page_of_window(ctx.window.id)
      show_page(ctx.state, target_page)

      -- Only floating windows can be interactively moved/resized in this config.
      if floating_for_window[ctx.window.id] and ctx.modifiers and ctx.modifiers.super then
        if ctx.button == BTN_LEFT then
          evil.window.begin_move(ctx.window.id)
        elseif ctx.button == BTN_RIGHT then
          evil.window.begin_resize(ctx.window.id, resize_edges_for_floating_window(ctx))
        end
      end

      return
    end

    return
  end

  if ctx.reason == "window_unmapped" then
    local next_window = page_focus_candidate(ctx.state, current_page)
    if next_window then
      evil.window.focus(next_window.id)
    else
      evil.window.clear_focus()
    end
  end
end

--------------------------------------------------------------------------------
-- Key helpers
--------------------------------------------------------------------------------

local function handle_page_number_shortcuts(ctx)
  local page_key = page_index_for_key(ctx.key)
  if not page_key then
    return false
  end

  if ctx.modifiers.super and ctx.modifiers.shift then
    return move_focused_window_to_page(ctx.state, page_key)
  end

  if ctx.modifiers.super then
    return show_page(ctx.state, page_key)
  end

  return false
end

local function handle_page_cycle_shortcuts(ctx)
  if ctx.modifiers.super and ctx.modifiers.shift and ctx.key == "H" then
    return move_focused_window_to_page(ctx.state, current_page - 1)
  end
  if ctx.modifiers.super and ctx.modifiers.shift and ctx.key == "L" then
    return move_focused_window_to_page(ctx.state, current_page + 1)
  end
  if ctx.modifiers.super and ctx.key == "H" then
    return show_page(ctx.state, current_page - 1)
  end
  if ctx.modifiers.super and ctx.key == "L" then
    return show_page(ctx.state, current_page + 1)
  end

  return false
end

local function handle_focus_cycle_shortcuts(ctx)
  if ctx.modifiers.super and ctx.key == "J" then
    return cycle_focus_on_page(ctx.state, 1)
  end
  if ctx.modifiers.super and ctx.key == "K" then
    return cycle_focus_on_page(ctx.state, -1)
  end

  return false
end

local function handle_mode_shortcuts(ctx)
  if ctx.keyspec == "Super+Space" then
    return toggle_floating(ctx.state)
  end
  if ctx.keyspec == "Super+F" then
    return toggle_fullscreen(ctx.state)
  end

  return false
end

local function handle_spawn_and_close_shortcuts(ctx)
  if ctx.keyspec == "Super+Return" then
    return evil.spawn(commands.terminal)
  end
  if ctx.keyspec == "Super+T" then
    return evil.spawn("kitty")
  end
  if ctx.keyspec == "Super+W" then
    return evil.spawn(commands.browser)
  end
  if ctx.keyspec == "Super+E" then
    return evil.spawn(commands.file_manager)
  end
  if ctx.keyspec == "Super+D" then
    return evil.spawn(commands.launcher)
  end
  if ctx.keyspec == "Super+X" then
    return evil.spawn(commands.x11_test)
  end
  if ctx.keyspec == "Super+Shift+S" then
    return evil.spawn(commands.screenshot)
  end
  if ctx.keyspec == "Super+Q" and ctx.state.focused_window_id then
    return evil.window.close(ctx.state.focused_window_id)
  end

  return false
end

--------------------------------------------------------------------------------
-- Main config table
--------------------------------------------------------------------------------

evil.config({
  backend = "winit",

  -- Lock the camera to screen-sized pages.
  canvas = {
    min_zoom = 1.0,
    max_zoom = 1.0,
    zoom_step = 1.0,
    pan_step = 0,
    allow_pointer_zoom = false,
    allow_middle_click_pan = false,
    allow_gesture_navigation = false,
  },

  draw = {
    stack = { "background", "windows", "window_overlay", "popups", "overlay", "cursor" },
    clear_color = { 0.08, 0.05, 0.12, 1.0 },
  },

  -- Disable remembered/client-default sizes so tiling owns the layout.
  window = {
    use_client_default_size = false,
    remember_sizes_by_app_id = false,
    hide_client_decorations = true,
  },

  placement = {
    default_size = { w = 900, h = 600 },
    padding = 0,
    cascade_step = { x = 0, y = 0 },
  },

  -- This example manages float/fullscreen/page behavior in Lua state instead of
  -- using built-in rules.
  rules = {},
})

--------------------------------------------------------------------------------
-- Autostart and bindings
--------------------------------------------------------------------------------

evil.autostart(commands.terminal)

-- App / utility binds
evil.bind("Super+Return", "spawn", { command = commands.terminal })
evil.bind("Super+T", "spawn", { command = "kitty" })
evil.bind("Super+W", "spawn", { command = commands.browser })
evil.bind("Super+E", "spawn", { command = commands.file_manager })
evil.bind("Super+Q", "close_window")
evil.bind("Super+D", "spawn", { command = commands.launcher })
evil.bind("Super+X", "spawn", { command = commands.x11_test })
evil.bind("Super+Shift+S", "spawn", { command = commands.screenshot })

-- Page navigation / movement binds.
-- These still use simple built-in bindings so the key hook always runs for the
-- same keys and can replace the normal canvas behavior.
evil.bind("Super+H", "pan_left", { amount = 1 })
evil.bind("Super+L", "pan_right", { amount = 1 })
evil.bind("Super+Shift+H", "pan_left", { amount = 1 })
evil.bind("Super+Shift+L", "pan_right", { amount = 1 })

-- Focus-cycle binds.
evil.bind("Super+J", "focus_next")
evil.bind("Super+K", "focus_prev")

-- Mode toggles.
evil.bind("Super+Space", "pan_left", { amount = 1 })
evil.bind("Super+F", "pan_left", { amount = 1 })

-- Direct page shortcuts.
for page = 1, 9 do
  evil.bind("Super+" .. tostring(page), "pan_left", { amount = 1 })
  evil.bind("Super+Shift+" .. tostring(page), "pan_left", { amount = 1 })
end
evil.bind("Super+0", "pan_left", { amount = 1 })
evil.bind("Super+Shift+0", "pan_left", { amount = 1 })

--------------------------------------------------------------------------------
-- Hook assignments
--------------------------------------------------------------------------------

evil.on.resolve_focus = tiling_resolve_focus

-- When a window appears, put it on the current page and split the hovered tile.
evil.on.window_mapped = function(ctx)
  page_for_window[ctx.window.id] = current_page
  insert_tiled_window_into_layout(ctx.state, current_page, ctx.window.id)
  relayout(ctx.state)
  evil.window.focus(ctx.window.id)
  show_page(ctx.state, current_page)
end

-- Clean up any Lua-owned state when a window disappears.
evil.on.window_unmapped = function(ctx)
  local page = page_for_window[ctx.window.id]
  if page then
    page_layout_roots[page] = remove_window_from_layout(page_layout_roots[page], ctx.window.id)
  end

  page_for_window[ctx.window.id] = nil
  floating_for_window[ctx.window.id] = nil
  floating_bounds[ctx.window.id] = nil

  for fullscreen_page, window_id in pairs(fullscreen_for_page) do
    if window_id == ctx.window.id then
      fullscreen_for_page[fullscreen_page] = nil
    end
  end

  relayout(ctx.state)
end

-- Keep the camera aligned with the newly focused window's page.
evil.on.focus_changed = function(ctx)
  if ctx.focused_window then
    local page = page_of_window(ctx.focused_window.id)
    relayout(ctx.state)
    show_page(ctx.state, page)
    return
  end

  relayout(ctx.state)
end

-- Floating windows can be moved freely.
evil.on.move_update = function(ctx)
  if not floating_for_window[ctx.window.id] then
    return
  end

  local bounds = {
    x = ctx.window.x + ctx.dx,
    y = ctx.window.y + ctx.dy,
    w = ctx.window.w,
    h = ctx.window.h,
  }

  floating_bounds[ctx.window.id] = bounds
  set_window_bounds(ctx.window.id, bounds)
end

-- Floating windows can also be resized freely.
evil.on.resize_update = function(ctx)
  if not floating_for_window[ctx.window.id] then
    return
  end

  local bounds = {
    x = ctx.window.x,
    y = ctx.window.y,
    w = ctx.window.w,
    h = ctx.window.h,
  }

  if ctx.edges.left then
    bounds.x = bounds.x + ctx.dx
    bounds.w = bounds.w - ctx.dx
  end
  if ctx.edges.right then
    bounds.w = bounds.w + ctx.dx
  end
  if ctx.edges.top then
    bounds.y = bounds.y + ctx.dy
    bounds.h = bounds.h - ctx.dy
  end
  if ctx.edges.bottom then
    bounds.h = bounds.h + ctx.dy
  end

  bounds.w = math.max(100, bounds.w)
  bounds.h = math.max(80, bounds.h)

  floating_bounds[ctx.window.id] = bounds
  set_window_bounds(ctx.window.id, bounds)
end

-- Handle the page/focus/mode/launcher shortcuts.
evil.on.key = function(ctx)
  if handle_page_number_shortcuts(ctx) then
    return
  end
  if handle_page_cycle_shortcuts(ctx) then
    return
  end
  if handle_focus_cycle_shortcuts(ctx) then
    return
  end
  if handle_mode_shortcuts(ctx) then
    return
  end
  handle_spawn_and_close_shortcuts(ctx)
end

-- Reuse the shared focus-border drawing helper so the example stays readable.
evil.on.draw_window_overlay = common.draw_focus_border
