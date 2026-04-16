# evilwm

> [!WARNING]
> `evilwm` is an unfinished research/prototype project. This GitHub mirror exists mainly as a public portfolio snapshot of the idea and direction. If you are not actively developing on it, you should treat it as preview material rather than something ready for installation, everyday use, or polished build/run guidance.

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

A typical config today combines a small amount of setup with hook-based policy:

```lua
evil.config({
  canvas = {
    min_zoom = 0.2,
    max_zoom = 4.0,
    zoom_step = 1.15,
    pan_step = 64,
  },
})

evil.bind("Super+H", "pan_left", { amount = 32 })

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
- Lua returns or triggers policy actions like moving, focusing, resizing, and drawing

This mirror includes one small self-contained example config:

- [`example-config.lua`](./example-config.lua)

It is included to show the current direction of the Lua API, not to imply long-term stability yet.

## Canonical repository

**This GitHub repository is a curated release mirror.**

For active development, full history, issues, more example configs, tests, detailed documentation, and the actual development workflow, use the canonical repository:

**https://git.evileko.dev/evileko/evilwm**

## Development / contributing

If you want to follow development or work on the project, start here:

**https://git.evileko.dev/evileko/evilwm**
