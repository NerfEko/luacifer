# evilwm

> [!WARNING]
> `evilwm` is an unfinished research/prototype project. This GitHub mirror exists mainly as a public portfolio snapshot of the idea and direction. If you are not actively developing on it, you should treat it as preview material rather than something ready for installation, everyday use, or polished build/run guidance.
>
> **Active development repo:** https://git.evileko.dev/evileko/evilwm

`evilwm` is an experimental Wayland compositor built around a shared infinite canvas: windows live in world coordinates, and outputs behave like cameras looking into that same world.

## The idea

Most desktop environments make monitors or workspaces the primary containers that windows live inside. `evilwm` goes in a different direction:

- windows exist in one shared 2D world
- outputs are viewports into that world
- navigation can feel more like panning around a map or canvas than switching between isolated workspaces
- window-manager behavior should be scriptable instead of hardcoded

The long-term goal is a compositor where:

- **Rust provides facts**
- **Lua provides policy**

In practice, that means Rust should own runtime truth, rendering, protocol handling, validation, and safety, while Lua should be able to shape focus rules, movement behavior, placement, layouts, grouping, and compositor-side visuals.

The point is not just “a compositor with a config file.” The point is a small compositor kernel that can support very different window-management styles without needing a fork for every personality.

## Why it exists

`evilwm` is an attempt to explore a desktop model that is:

- more spatial than workspace-driven
- more composable than monolithic window managers
- more scriptable in policy without giving up a strict runtime core

If it succeeds, the same core could support a freeform floating workflow, a Lua-authored tiler, a spatial canvas desktop, or hybrids that borrow from all three.

## Current status

Today, the project is best read as:

1. a compositor architecture experiment
2. a growing policy/runtime testbed
3. an early prototype, not a finished user product

Active development is ongoing, and the real source of truth lives on Forgejo.

## What the Lua layer is for

The Lua side is where `evilwm` is supposed to become expressive.

Rather than hardcoding all window-manager behavior in Rust, the project exposes runtime facts to Lua and lets Lua decide policy. That includes things like:

- keybindings
- focus behavior
- movement and resize rules
- placement logic
- compositor-drawn overlays and visuals

A typical config today combines setup, hook-based policy, and drawing:

```lua
evil.config({
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },
})

evil.bind("Super+Return", "spawn", { command = terminal_cmd })

evil.on.place_window = function(ctx)
  return { kind = "set_bounds", id = ctx.window.id, x = 200, y = 160, w = 960, h = 720 }
end

evil.on.move_update = function(ctx)
  return {
    kind = "move_window",
    id = ctx.window.id,
    x = ctx.window.x + ctx.dx,
    y = ctx.window.y + ctx.dy,
  }
end
```

The important part is the shape of the system:

- Rust provides authoritative state and validated primitive actions
- Lua reads runtime facts through hook context (`ctx`)
- Lua returns or triggers policy actions like moving, focusing, resizing, placing, spawning, and drawing

This mirror includes one fuller self-contained example config:

- [`example-config.lua`](./example-config.lua)

It is meant to be a readable baseline with practical keybindings, spawn commands, placement, focus behavior, movement, resize handling, and compositor-drawn visuals.

## Why this is the interesting part

The real promise of this project is not just “Lua configuration.” It is that **a Lua config can completely change the kind of window manager this compositor feels like, without requiring a fork of the Rust core.**

That means the same compositor kernel could eventually support very different personalities just by changing policy and helper modules.

<details>
<summary><strong>Spatial floating canvas setup</strong></summary>

This is the direction the current public example config leans toward: windows cascade into view, movement stays freeform, and the canvas itself is part of the workflow.

```lua
evil.bind("Super+H", "pan_left", { amount = 48 })
evil.bind("Super+L", "pan_right", { amount = 48 })
evil.bind("Super+Equal", "zoom_in", { amount = 1.15 })

evil.on.place_window = function(ctx)
  return {
    kind = "set_bounds",
    id = ctx.window.id,
    x = ctx.window.x + 48,
    y = ctx.window.y + 48,
    w = 960,
    h = 720,
  }
end

evil.on.move_update = function(ctx)
  return {
    kind = "move_window",
    id = ctx.window.id,
    x = ctx.window.x + ctx.dx,
    y = ctx.window.y + ctx.dy,
  }
end
```
</details>

<details>
<summary><strong>Classic tiler personality</strong></summary>

A different config could use the same compositor as a much more layout-driven WM by making placement deterministic and treating map events like relayout opportunities.

```lua
local next_column = 0

evil.on.place_window = function(ctx)
  local x = next_column * 640
  next_column = (next_column + 1) % 2
  return {
    kind = "set_bounds",
    id = ctx.window.id,
    x = x,
    y = 0,
    w = 640,
    h = 720,
  }
end

evil.on.window_mapped = function(ctx)
  return { kind = "focus_window", id = ctx.window.id }
end
```
</details>

<details>
<summary><strong>Automation-heavy personal desktop</strong></summary>

Lua can also encode personal workflow rules directly in config logic.

```lua
evil.on.place_window = function(ctx)
  if ctx.window.app_id == "foot" then
    return { kind = "set_bounds", id = ctx.window.id, x = 80, y = 80, w = 900, h = 620 }
  end

  if ctx.window.title and string.find(string.lower(ctx.window.title), "music", 1, true) then
    return { kind = "set_bounds", id = ctx.window.id, x = 1200, y = 120, w = 520, h = 520 }
  end
end

evil.on.focus_changed = function(ctx)
  if ctx.focused_window then
    print("focused:", ctx.focused_window.title or ctx.focused_window.app_id or ctx.focused_window.id)
  end
end
```
</details>

<details>
<summary><strong>Hybrid setups</strong></summary>

A hybrid config can mix floating, layout-style placement, and policy exceptions without splitting into separate compositor forks.

```lua
evil.on.place_window = function(ctx)
  if ctx.window.app_id == "pavucontrol" then
    return { kind = "set_bounds", id = ctx.window.id, x = 1200, y = 160, w = 420, h = 520 }
  end

  return { kind = "set_bounds", id = ctx.window.id, x = 160, y = 120, w = 1000, h = 720 }
end

evil.on.draw_overlay = function(ctx)
  if not ctx.focused_window then
    return nil
  end

  return evil.draw.stroke_rect({
    space = "world",
    x = ctx.focused_window.x,
    y = ctx.focused_window.y,
    w = ctx.focused_window.w,
    h = ctx.focused_window.h,
    width = 3,
    color = { 0.74, 0.58, 0.98, 0.98 },
  })
end
```
</details>

## Active development

**This GitHub repository is a curated release mirror.**

For active development, full history, issues, more example configs, tests, detailed documentation, and the real development workflow, use the main development repo:

**https://git.evileko.dev/evileko/evilwm**

## Development / contributing

If you want to follow development or work on the project, start here:

**https://git.evileko.dev/evileko/evilwm**
