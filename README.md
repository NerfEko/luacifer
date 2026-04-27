# Luacifer

> [!NOTE]
> This GitHub repository is a **curated release mirror**.
>
> Active development, issue tracking, and full documentation live on Forgejo:
>
> - **Main repo:** https://git.evileko.dev/evileko/luacifer
> - **Project wiki:** https://git.evileko.dev/evileko/luacifer/wiki

---

> _Summon your desktop. Possess it with Lua. Let the evil in._

**Luacifer** is a Wayland compositor built on an infinite canvas. Every window lives in world coordinates. Every output is a camera. You write the rules in Lua — and the engine executes them.

The guiding philosophy is simple:

> **Rust provides facts. Lua provides policy.**

Luacifer handles the hard stuff: Wayland protocols, rendering, input devices, DRM. You write a Lua script that says _where_ windows go, _how_ they move, and _what_ happens when you press a key. No recompiling. No patching C. That's the E.V.I.L. engine at work.

**Luacifer** = **Lua** + **Lucifer** (the light-bringer). It lights up your displays. It also brings a little chaos, in the best way.

### The E.V.I.L. engine — Event & Viewport Integration Layer

Under the hood, Luacifer runs on the **E.V.I.L.** engine — the pure core that owns the canvas, viewport, window model, focus, placement, and input binding. Every config script taps into it through the `evil.*` Lua namespace:

- `evil.config()` — set up your compositor in one table
- `evil.window` — query, move, resize, and focus windows
- `evil.canvas` — pan and zoom the infinite world-space viewport
- `evil.bind()` — wire keys to Lua actions
- `evil.on.*` hooks — intercept focus, placement, and lifecycle events

The engine is also what makes the headless runtime possible: you can write and test window management policy without a display, then run the same config on real hardware.

---

## Quick start

```bash
# Build it
cargo build

# Validate a config (catches mistakes before you run)
cargo run --bin luacifer -- --check-config --config example-config.lua

# Run in a window (nested compositor — great for hacking)
cargo run --bin luacifer -- --backend winit --config example-config.lua

# Spawn a terminal inside the nested compositor
cargo run --bin luacifer -- --backend winit --config example-config.lua --command foot

# Run headless (no display needed — perfect for testing)
cargo run --bin luacifer -- --backend headless --config example-config.lua

# Brave? Try it on real hardware (spare VT only!)
cargo run --bin luacifer --release --features udev -- --backend udev --config example-config.lua
```

---

## Example configs

This mirror ships the full set of example configs — pick one as a starting point:

| Config                       | What it does                                              |
| ---------------------------- | --------------------------------------------------------- |
| `example-config.lua`         | Standalone public baseline — self-contained, tty-targeted |
| `examples/config.lua`        | Minimal baseline — keybinds, spawn rules, basic focus     |
| `examples/tiling.lua`        | Dynamic tiling — windows arrange themselves               |
| `examples/freeform-move.lua` | Float everything, drag freely                             |
| `examples/snap-grid.lua`     | Grid-assisted placement with snapping                     |
| `examples/clump.lua`         | Window clumping / packing layout                          |
| `examples/nested-debug.lua`  | Debug-oriented profile for nested runs                    |
| `examples/tty-baseline.lua`  | Minimal TTY profile for bare-metal testing                |
| `examples/rules.lua`         | Rule-driven placement and behavior                        |

Dive deeper: **[Example Configs Overview →](https://git.evileko.dev/evileko/luacifer/wiki/Example-Configs-Overview)**

---

## What you can do today

Luacifer is functional and fun _right now_:

- **Script your desktop in Lua.** Focus, placement, keybindings, resize — all configurable without touching Rust.
- **Iterate fast.** The nested winit backend runs in a window. Edit your config, restart, repeat.
- **Test deterministically.** The headless runtime lets you write regression tests for window management policy.
- **Go bare metal.** The udev/DRM backend runs on a real TTY (experimental, but already real).

---

## Development

```bash
# Build
cargo build

# Run tests
cargo test --all-targets
cargo test --no-default-features

# Lint
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check

# Validate all shipped configs
cargo run --bin luacifer -- --check-config --config example-config.lua
for cfg in examples/*.lua; do
  cargo run --bin luacifer -- --check-config --config "$cfg"
done
```

### Feature flags

| Combination                         | What you get                       | CI  |
| ----------------------------------- | ---------------------------------- | --- |
| `--no-default-features`             | Pure core — no compositor, no Lua  | ✅  |
| default (`winit,lua,xwayland`)      | Nested compositor + Lua + Xwayland | ✅  |
| `--features lua,udev` (no defaults) | Standalone TTY/DRM + Lua           | ✅  |
| `--all-features`                    | Everything at once                 | ✅  |

---

## Documentation

All the real docs live on the Forgejo wiki:

| Page                                                                                                 | What's in it                     |
| ---------------------------------------------------------------------------------------------------- | -------------------------------- |
| [Writing Your First Config](https://git.evileko.dev/evileko/luacifer/wiki/Writing-Your-First-Config) | Start here if you're new         |
| [Lua API Guide](https://git.evileko.dev/evileko/luacifer/wiki/Lua-API-Guide)                         | Complete `evil.*` API reference  |
| [Lua API Cheat Sheet](https://git.evileko.dev/evileko/luacifer/wiki/Lua-API-Cheat-Sheet)             | Quick reference card             |
| [Hooks Guide](https://git.evileko.dev/evileko/luacifer/wiki/Hooks-Guide)                             | Event hooks explained            |
| [Example Configs Overview](https://git.evileko.dev/evileko/luacifer/wiki/Example-Configs-Overview)   | Walkthroughs of every example    |
| [Example Tiling](https://git.evileko.dev/evileko/luacifer/wiki/Example-Tiling)                       | Deep dive into the tiling config |
| [Testing & Debugging](https://git.evileko.dev/evileko/luacifer/wiki/Testing-and-Debugging)           | How to test and debug            |
| [Feature Status](https://git.evileko.dev/evileko/luacifer/wiki/Feature-Status-and-Limitations)       | What works, what's coming        |

---

## What this mirror is

This branch is a curated public snapshot for GitHub. Use it to browse the source, build the compositor, or try the configs.

For the full development history, issue tracking, and contributor workflow:

- **Main repo:** https://git.evileko.dev/evileko/luacifer
- **Wiki:** https://git.evileko.dev/evileko/luacifer/wiki

---

## License

MIT © EKo
