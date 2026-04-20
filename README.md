# evilwm

> [!NOTE]
> This GitHub repository is a **curated release mirror**.
>
> Active development, issue tracking, and long-form documentation live on Forgejo:
>
> - **Main repo:** https://git.evileko.dev/evileko/evilwm
> - **Project wiki:** https://git.evileko.dev/evileko/evilwm/wiki

`evilwm` is an experimental Wayland compositor built around a shared infinite canvas: windows live in world coordinates, and outputs behave like cameras looking into that same world.

It is aimed much more at **developers who want to author their own desktop behavior** than at users looking for a polished default environment.

The core project rule is:

- **Rust provides facts**
- **Lua provides policy**

---

## Main documentation lives in the Forgejo wiki

If you want the real documentation for the project — especially if you want to learn the Lua API, understand the example configs, or craft your own Lua scripts — use the wiki:

- **Wiki home:** https://git.evileko.dev/evileko/evilwm/wiki
- **Writing your first config:** https://git.evileko.dev/evileko/evilwm/wiki/Writing-Your-First-Config
- **Lua API guide:** https://git.evileko.dev/evileko/evilwm/wiki/Lua-API-Guide
- **Example configs overview:** https://git.evileko.dev/evileko/evilwm/wiki/Example-Configs-Overview
- **Tiling example walkthrough:** https://git.evileko.dev/evileko/evilwm/wiki/Example-Tiling
- **Testing and debugging:** https://git.evileko.dev/evileko/evilwm/wiki/Testing-and-Debugging

If your goal is **writing or editing Lua configs**, start here:

- https://git.evileko.dev/evileko/evilwm/wiki/Writing-Your-First-Config

---

## What evilwm is today

Today, `evilwm` is best understood as three things at once:

1. a **pure core library** for canvas, viewport, focus, placement, rules, resize, binding, and output logic
2. a **deterministic headless runtime** for policy development and regression testing
3. a **real live compositor** with a practical nested `winit` path and an early standalone `udev` / tty path

## Current state

`evilwm` is already useful if your goal is to:

- script your own desktop behavior in Lua
- experiment with focus, movement, resize, and placement policy
- iterate on custom workflows against a headless runtime and a real nested compositor
- build on a project that already has real example configs, live smoke coverage, and an actively growing Lua surface

The main areas still evolving are:

- standalone tty confidence and recovery behavior
- broader desktop / protocol coverage beyond the current proven slices
- long-term Lua API stabilization and cleanup

For the detailed feature status and limitations, use the wiki:

- https://git.evileko.dev/evileko/evilwm/wiki/Feature-Status-and-Limitations

## Feature overview

| Area | Current picture |
| --- | --- |
| Core canvas / window / output logic | strong foundation |
| Headless runtime | practical and heavily testable |
| Nested `winit` compositor | the main live path and a good place to actually use and iterate on configs |
| Lua config + hook surface | real, useful, and expanding |
| Example configs | multiple serious starting points, including tiling and tty-focused profiles |
| TTY / standalone backend | usable for controlled testing, still the roughest part of the project |
| Desktop / protocol coverage | several important slices are real already, with broader coverage still growing |
| Documentation | wiki-first, with detailed guides for config writing, API use, examples, and debugging |

---

## Quick start

This release mirror ships one self-contained public example config:

- `example-config.lua`

### Validate the config

```bash
cargo run --bin evilwm -- --check-config --config example-config.lua
```

### Run headless

```bash
cargo run --bin evilwm -- --backend headless --config example-config.lua
```

### Run the nested compositor

```bash
cargo run --bin evilwm -- --backend winit --config example-config.lua
```

### Spawn a client into the nested compositor

```bash
cargo run --bin evilwm -- --backend winit --config example-config.lua --command foot
```

### Try the current tty path on a spare VT

```bash
cargo run --bin evilwm --release --features udev -- --backend udev --config example-config.lua
```

---

## What this mirror is for

This branch exists as a smaller public snapshot of the project.

Use it if you want to:

- browse a compact source snapshot
- build the compositor and try the public example config
- share or reference the project from GitHub

Use the Forgejo repo and wiki if you want:

- the full development history
- the full set of example configs
- long-form documentation and status pages
- current contributor workflow and project planning

- **Main repo:** https://git.evileko.dev/evileko/evilwm
- **Wiki:** https://git.evileko.dev/evileko/evilwm/wiki
