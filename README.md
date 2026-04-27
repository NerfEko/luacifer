# Luacifer

> [!NOTE]
> This GitHub repository is a **curated release mirror**.
>
> Active development, issue tracking, and full documentation live on Forgejo:
>
> - **Main repo:** https://git.evileko.dev/evileko/luacifer
> - **Project wiki:** https://git.evileko.dev/evileko/luacifer/wiki

---

> _A compositor so nice, you configure it with your evil twin._

**Luacifer** is a Wayland compositor built on an infinite canvas. Every window lives in world coordinates. Every output is a camera. You write the rules in Lua — and the engine executes them.

The guiding philosophy is simple:

> **Rust provides facts. Lua provides policy.**

Luacifer handles the hard stuff: Wayland protocols, rendering, input devices, DRM. You write a Lua script that says _where_ windows go, _how_ they move, and _what_ happens when you press a key. No recompiling. No patching C. That's the E.V.I.L. engine at work.

**Luacifer** = **Lua** + **Lucifer** (the light-bringer). It lights up your displays. It also brings a little chaos, in the best way.

**E.V.I.L.** — Event & Viewport Integration Layer.

---

## Quick start

This mirror ships one self-contained public example config:

- `example-config.lua`

```bash
# Build it
cargo build

# Validate the config
cargo run --bin luacifer -- --check-config --config example-config.lua

# Run in a window (nested compositor — great for hacking)
cargo run --bin luacifer -- --backend winit --config example-config.lua

# Spawn a terminal inside
cargo run --bin luacifer -- --backend winit --config example-config.lua --command foot

# Run headless (no display needed)
cargo run --bin luacifer -- --backend headless --config example-config.lua

# Brave? Try on a spare VT
cargo run --bin luacifer --release --features udev -- --backend udev --config example-config.lua
```

---

## What you can do today

Luacifer is functional and fun _right now_:

- **Script your desktop in Lua.** Focus, placement, keybindings, resize — all configurable without touching Rust.
- **Iterate fast.** The nested winit backend runs in a window. Edit your config, restart, repeat.
- **Test deterministically.** The headless runtime lets you write regression tests for window management policy.
- **Go bare metal.** The udev/DRM backend runs on a real TTY (experimental, but already real).

---

## Documentation

All the real docs live on the Forgejo wiki:

| Page                                                                                                 | What's in it                    |
| ---------------------------------------------------------------------------------------------------- | ------------------------------- |
| [Writing Your First Config](https://git.evileko.dev/evileko/luacifer/wiki/Writing-Your-First-Config) | Start here if you're new        |
| [Lua API Guide](https://git.evileko.dev/evileko/luacifer/wiki/Lua-API-Guide)                         | Complete `evil.*` API reference |
| [Hooks Guide](https://git.evileko.dev/evileko/luacifer/wiki/Hooks-Guide)                             | Event hooks explained           |
| [Example Configs Overview](https://git.evileko.dev/evileko/luacifer/wiki/Example-Configs-Overview)   | Walkthroughs of every example   |
| [Feature Status](https://git.evileko.dev/evileko/luacifer/wiki/Feature-Status-and-Limitations)       | What works, what's coming       |
| [Testing & Debugging](https://git.evileko.dev/evileko/luacifer/wiki/Testing-and-Debugging)           | How to test and debug           |

---

## What this mirror is for

This branch is a smaller public snapshot for GitHub. Use it if you want to browse a compact source tree, build the compositor, or try the public example config.

For the full development history, all example configs, long-form docs, and the contributor workflow:

- **Main repo:** https://git.evileko.dev/evileko/luacifer
- **Wiki:** https://git.evileko.dev/evileko/luacifer/wiki
