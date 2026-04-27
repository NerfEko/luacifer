-- examples/clump.lua
--
-- This example changes only one thing:
-- when a new window appears, try to place it next to another window instead of
-- using the default fallback placement.
--
-- It is a small example of a placement policy hook.

--------------------------------------------------------------------------------
-- Shared data
--------------------------------------------------------------------------------

local shared_rules = include("rules.lua")

--------------------------------------------------------------------------------
-- User-tunable settings
--------------------------------------------------------------------------------

local GAP = 24

--------------------------------------------------------------------------------
-- Placement helpers
--------------------------------------------------------------------------------

local function rectangles_overlap(a, b)
  return a.x + a.w > b.x
    and a.x < b.x + b.w
    and a.y + a.h > b.y
    and a.y < b.y + b.h
end

local function overlaps_any_existing_window(candidate, windows, ignore_id)
  for _, window in ipairs(windows) do
    if window.id ~= ignore_id then
      local other = {
        x = window.x,
        y = window.y,
        w = window.w,
        h = window.h,
      }

      if rectangles_overlap(candidate, other) then
        return true
      end
    end
  end

  return false
end

local function candidate_positions(anchor, current)
  return {
    { x = anchor.x + anchor.w + GAP, y = anchor.y, w = current.w, h = current.h },
    { x = anchor.x, y = anchor.y + anchor.h + GAP, w = current.w, h = current.h },
    { x = anchor.x - current.w - GAP, y = anchor.y, w = current.w, h = current.h },
    { x = anchor.x, y = anchor.y - current.h - GAP, w = current.w, h = current.h },
  }
end

local function find_anchor_window(ctx)
  local fallback = nil

  for _, window in ipairs(ctx.state.windows) do
    if window.id ~= ctx.window.id then
      if window.focused then
        return window
      end
      fallback = fallback or window
    end
  end

  return fallback
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

evil.on.place_window = function(ctx)
  local anchor = find_anchor_window(ctx)
  if not anchor then
    return
  end

  for _, candidate in ipairs(candidate_positions(anchor, ctx.window)) do
    if not overlaps_any_existing_window(candidate, ctx.state.windows, ctx.window.id) then
      evil.window.move(ctx.window.id, candidate.x, candidate.y)
      return
    end
  end

  evil.window.move(ctx.window.id, anchor.x + anchor.w + GAP, anchor.y)
end
