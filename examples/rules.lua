-- examples/rules.lua
--
-- Shared example rules.
--
-- These are intentionally tiny. They exist to show the shape of rules without
-- burying the examples in lots of policy.

return {
  -- Give foot a comfortable default floating size in the example configs.
  { app_id = "foot", floating = true, size = { w = 900, h = 600 } },

  -- Windows with "scratch" in the title are ignored by focus logic.
  { title_contains = "scratch", exclude_from_focus = true },
}
