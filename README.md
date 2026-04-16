# evilwm

`evilwm` is an experimental Wayland compositor built around a shared infinite canvas: windows live in world coordinates, and outputs behave like cameras looking into that same world.

## Status

`evilwm` is currently a prototype. It is not yet a finished daily-driver compositor.

## Canonical repository

**This GitHub repository is a curated release mirror.**

For active development, full history, issues, examples, tests, and detailed documentation, use the canonical repository:

**https://git.evileko.dev/evileko/evilwm**

## Build

```bash
cargo build
```

## Run

This public mirror is intentionally trimmed down, so the simplest way to try it is without a config:

Nested prototype:

```bash
cargo run -- --backend winit --no-config
```

Headless mode:

```bash
cargo run -- --backend headless --no-config
```

For the full development tree, example configs, experimental tty workflow, and current project details, see:

**https://git.evileko.dev/evileko/evilwm**

## Development / contributing

Please use the canonical repository for development, issues, and documentation:

**https://git.evileko.dev/evileko/evilwm**
