# luacifer issue plan

> Status: **P0 is complete as of 2026-04-24**.
>
> Completed P0 work:
> - unified live output/camera state around a single map-backed model with a bootstrap fallback entry
> - grouped major live compositor state into runtime sub-structs inside `EvilWm`
> - replaced the live Lua `take()/reinsert` ownership workaround with shared hook ownership
> - verified/fixed SHM format advertisement across live backends
> - made nested `winit` redraw scheduling explicit instead of effectively continuous

This file turns the review findings into a prioritized work plan.

It is intentionally opinionated: priority is based on **correctness risk**, **architecture drag**, **daily-driver impact**, **future feature cost**, and **ease of introducing subtle bugs**.

## Priority legend

- **P0 — fix first**: correctness, architecture, or runtime problems likely to cause bugs, block feature work, or create expensive refactors later.
- **P1 — high value soon**: important product/runtime gaps that are not immediate blockers but materially affect usability or maintainability.
- **P2 — useful improvements**: worthwhile fixes that improve polish, debuggability, or developer experience.
- **P3 — cleanup / code health**: small but real issues that should be addressed opportunistically.

---

## P0 — fix first

### 1. Split output state model (`output_state` + `output_states`)

**Priority:** P0

**What is wrong**

The compositor currently carries two overlapping representations of output/camera state:

- `self.output_state` — a single fallback `OutputState`
- `self.output_states` — a `BTreeMap<String, OutputState>` keyed by output name

A large amount of code falls back from one to the other. Some paths look at the primary output in the map, some read the single fallback field, and some update both. This strongly suggests the codebase is midway between a single-output architecture and a true multi-output architecture.

**Why this matters**

This is the most dangerous design issue in the project because outputs are not just monitors here — they are also the camera/view abstraction over the infinite canvas. If output state is duplicated, then the compositor can silently disagree with itself about:

- what the primary viewport is
- where pointer coordinates should be interpreted
- which output a window belongs to
- where layer-shell surfaces should render
- what should be reported through IPC and Lua snapshots

That kind of disagreement tends to produce bugs that are hard to reproduce: wrong output associations, stale camera state after hotplug, pointer math that only fails on one backend, and focus/render bugs that appear only in multi-monitor or tty mode.

**Likely symptoms if left alone**

- output add/remove bugs
- stale primary viewport state
- incorrect output IDs in snapshots
- inconsistent pan/zoom behavior across outputs
- hidden logic forks where single-output and multi-output paths evolve differently

**Recommended direction**

Pick one model and remove the other.

Best direction: make `output_states` the single source of truth and treat “primary output” as a query, not a duplicated cached object. If a fallback is needed before outputs exist, use a dedicated bootstrap state type instead of a permanent second representation.

**Concrete steps**

1. Identify every read/write of `self.output_state`.
2. Categorize each call site as bootstrap-only, legacy single-output behavior, or accidental duplication.
3. Introduce helper APIs like `primary_output_state()`, `primary_output_state_mut()`, and `iter_output_states()`.
4. Migrate all runtime logic to the map-based model.
5. Delete `self.output_state` once all paths are covered.
6. Add tests for single-output, multi-output, hotplug/removal, and tty layout transitions.

**Risk if changed carelessly**

Medium-high. This touches pointer math, rendering, and snapshot generation. But the longer it stays, the more expensive every later output-related change becomes.

---

### 2. `EvilWm` is a god struct

**Priority:** P0

**What is wrong**

`EvilWm` owns nearly everything directly: compositor state, seat state, outputs, windows, rules, IPC, event logging, tty lifecycle state, Lua integration, interactive operations, screenshot state, XWayland state, and more.

The result is a very wide state object with many responsibilities and many partially coupled subsystems.

**Why this matters**

This is not just a style complaint. In a compositor, a giant state object has real costs:

- it makes invariant boundaries unclear
- it encourages unrelated code to reach into shared mutable state
- it pushes more logic into “utility methods” on the root object
- it increases borrow-checker friction, which often leads to workaround patterns rather than better structure
- it makes it harder to reason about ownership around Lua hooks, tty lifecycle, output state, and input routing

The current `with_live_lua(...take()...)` pattern is almost certainly downstream of this problem.

**Likely symptoms if left alone**

- harder refactors
- more accidental coupling between subsystems
- growing method sprawl across files
- increasing borrow-checker workarounds
- harder onboarding for future contributors

**Recommended direction**

Keep `EvilWm` as the trait-implementation shell required by Smithay, but move grouped state into dedicated sub-structs.

Suggested splits:

- `WindowRuntimeState`
- `OutputRuntimeState`
- `InputRuntimeState`
- `LuaRuntimeState`
- `TtyRuntimeState`
- `ObservabilityState` (event log / IPC trace / screenshot requests)

This allows the root compositor object to remain the integration point without being the storage location for every concern.

**Concrete steps**

1. Create internal sub-structs without changing behavior.
2. Move fields in small batches.
3. Add thin accessors to avoid massive churn.
4. Only after grouping state, revisit method placement and hook ownership.

**Risk if changed carelessly**

Medium. The work is mechanical but broad. The key is to do it in phases and keep tests green after each batch.

---

### 3. `with_live_lua` ownership pattern (`take` / call / reinsert)

**Priority:** P0

**What is wrong**

Live Lua hooks are accessed by temporarily removing them from `Option<LiveLuaHooks>`, calling into them, and then putting them back. This is a borrow-checker workaround.

**Why this matters**

It works, but it is brittle:

- it makes hook availability depend on control flow discipline
- it couples hook invocation style to the shape of `EvilWm`
- if the call path ever becomes more complex, it is easy to accidentally create “hooks missing” states
- it hides the real architectural problem: the compositor and hook runtime want narrower, more explicit access boundaries

Even if panics are rare, the pattern increases conceptual complexity everywhere hooks are used.

**Likely symptoms if left alone**

- harder-to-read hook code
- subtle refactor hazards
- awkward recursion / reentrancy decisions
- more logic built around “can I temporarily take this field?”

**Recommended direction**

Fix this after or alongside the god-struct cleanup.

Possible approaches:

1. Move hook runtime into its own interior-mutable sub-struct.
2. Pass immutable hook runtime + explicit mutable state slices into hook execution.
3. Introduce a dedicated hook executor object that owns Lua and receives narrow action/state adapters.

Best long-term direction: make hook execution depend on a small `HookHost`/`HookContext` abstraction rather than the whole compositor.

**Concrete steps**

1. Define what hook execution actually needs.
2. Extract snapshot building and action application boundaries.
3. Replace `Option<LiveLuaHooks>` + `take()` with a dedicated runtime holder.
4. Add tests around nested hook-triggering scenarios and error paths.

**Risk if changed carelessly**

Medium. The pattern is central but isolated enough to refactor once the surrounding state is better grouped.

---

### 4. SHM format advertisement likely incomplete

**Priority:** P0

**What is wrong**

`ShmState::new::<Self>(&dh, vec![])` is initialized with an empty format list. If formats are not updated later from the renderer, wl_shm clients may not learn what shared-memory buffer formats the compositor accepts.

**Why this matters**

This is a protocol-level correctness problem, not a polish issue. Some clients rely on proper SHM format advertisement to create software-rendered buffers. If formats are missing or incomplete:

- some clients may fail to render
- fallback rendering paths may break
- bugs may appear only for specific toolkit/client combinations

That kind of issue is especially nasty because it can look like “random client incompatibility” rather than a compositor bug.

**Recommended direction**

Verify whether formats are updated elsewhere. If not, wire renderer-supported shm formats into `ShmState` during backend initialization.

**Concrete steps**

1. Confirm current smithay behavior in this code path.
2. On renderer creation, query supported SHM formats.
3. Update `ShmState` accordingly.
4. Add a smoke test with a known shm client path.

**Risk if changed carelessly**

Low-medium. This should be straightforward once the correct Smithay API hook point is identified.

---

### 5. Nested backend appears to redraw continuously

**Priority:** P0

**What is wrong**

The winit event loop requests another redraw at the end of every `Redraw` event. The compositor also has a `redraw_requested` flag, but the loop does not seem to use it to skip unnecessary rendering.

**Why this matters**

An always-redrawing compositor has several hidden costs:

- unnecessary CPU/GPU usage while idle
- harder power/perf analysis
- noisier debugging of real redraw triggers
- performance differences between nested and tty backends become harder to compare fairly

For a project experimenting with camera motion and custom drawing, a clean redraw model matters.

**Recommended direction**

Convert redraw into an explicit invalidation model:

- request redraw on state changes
- only schedule another frame when animations, gestures, or frame callbacks require it
- keep a “force continuous redraw” mode only for debugging if needed

**Concrete steps**

1. Audit all `request_redraw()` call sites.
2. Gate nested redraw submission on `redraw_requested()`.
3. Clear the flag after successful render.
4. Re-request redraw only when needed.
5. Add an idle test or event-log assertion for redraw frequency.

**Risk if changed carelessly**

Medium. It can expose hidden assumptions where some state changes relied on the unconditional redraw loop to appear visually.

---

## P1 — high value soon

### 6. Session lock is not a real lock screen

**Priority:** P1

**What is wrong**

The current lock behavior is essentially:

- intercept input
- draw a dark overlay
- expose lock/unlock over IPC

It is not an authentication boundary.

**Why this matters**

This is mostly a product/trust issue. Calling this “lock” suggests a security property that the code does not actually provide. For an experimental compositor that may be acceptable internally, but once others use it, this becomes misleading.

**Recommended direction**

Decide explicitly between:

- a true lock integration (external locker or auth-capable surface flow)
- or a rename/reframe as “input shield” / “debug lock overlay” until it is real

If a true lock is out of scope for now, the safest move is honest naming.

**Concrete steps**

1. Document intended semantics internally in code comments.
2. Rename user-visible IPC/messages if security is not guaranteed.
3. Or integrate a real lock workflow.

**Risk if changed carelessly**

Low.

---

### 7. TTY backend cursor is prototype-grade

**Priority:** P1

**What is wrong**

The tty backend draws a plain square cursor in software and does not appear to support cursor themes or hardware cursor planes.

**Why this matters**

This affects real usability:

- poor visual quality
- no theme consistency
- potential latency/perf cost from software-rendered cursor
- reduces confidence in tty mode as a serious backend

**Recommended direction**

Treat this as part of making tty mode credible for daily use.

Possible phases:

1. themed software cursor
2. proper hotspot handling
3. hardware cursor support where available

**Risk if changed carelessly**

Medium because DRM cursor handling can be backend-specific.

---

### 8. No obvious damage tracking in tty backend

**Priority:** P1

**What is wrong**

The nested backend uses `OutputDamageTracker`. The tty backend does not appear to have equivalent incremental redraw management.

**Why this matters**

Full-frame redraws are acceptable for a prototype but become expensive as soon as:

- outputs get larger
- more windows are visible
- draw hooks add overlays
- cursor motion is frequent

This is especially important for a compositor whose viewport model invites panning and zooming over a potentially large world.

**Recommended direction**

Bring tty rendering closer to the nested backend’s discipline by introducing damage-aware rendering and measuring the impact.

**Concrete steps**

1. Verify whether DRM compositor internals already reduce some work.
2. Track invalidation regions at the compositor level.
3. Align damage policy across nested and tty modes where possible.
4. Measure idle and motion costs before/after.

**Risk if changed carelessly**

Medium-high because DRM pipelines are easy to destabilize.

---

### 9. Hand-rolled output management protocol is high-maintenance

**Priority:** P1

**What is wrong**

`output_management_protocol.rs` is a large, custom implementation of the output-management protocol.

**Why this matters**

It is impressive, but it creates maintenance burden:

- more protocol surface area to keep correct
- harder upgrades when Smithay/protocol details change
- more custom state synchronization logic to debug

This is the kind of code that often works until a protocol edge case, then becomes very expensive to reason about.

**Recommended direction**

Reduce custom protocol logic if a higher-level Smithay path exists. If not, isolate it further and aggressively test it.

**Concrete steps**

1. Re-evaluate available Smithay abstractions.
2. If custom is still necessary, narrow the module’s API surface.
3. Add targeted tests for enable/disable/move/reconfigure flows.

**Risk if changed carelessly**

Medium.

---

### 10. Probe/test client code is deeply embedded in the main crate

**Priority:** P1

**What is wrong**

The probe code is substantial and handles real protocol behaviors. It is useful, but it increases the scope of the compositor crate.

**Why this matters**

This has two costs:

- mental cost: the crate contains both compositor implementation and nontrivial client implementation
- build/test cost: probe support can become entangled with production code concerns

**Recommended direction**

Split probe utilities into a dedicated internal crate or test-support crate once the API stabilizes.

**Risk if changed carelessly**

Low-medium.

---

## P2 — useful improvements

### 11. Screenshot output uses PPM only

**Priority:** P2

**What is wrong**

Screenshots are written as PPM for simplicity and testability.

**Why this matters**

This is a practical usability problem, not a core architecture problem. PPM is huge and inconvenient outside debugging.

**Recommended direction**

Keep PPM as the zero-dependency debug/test format if desired, but add PNG support for normal use.

**Risk if changed carelessly**

Low.

---

### 12. Hook error reporting is safe but not very developer-friendly

**Priority:** P2

**What is wrong**

Hook errors are deduplicated and only printed on first occurrence, on message change, or every tenth repeat.

**Why this matters**

This reduces log spam, but it can hide important context while actively developing hooks. For a Lua-driven compositor, fast hook debugging is a core workflow.

**Recommended direction**

Support both modes:

- production-friendly deduplicated logging
- developer mode with full hook error emission and maybe richer context

**Risk if changed carelessly**

Low.

---

### 13. Feature naming / behavior around `x11` is confusing

**Priority:** P2

**What is wrong**

The Cargo feature name suggests a distinct backend path, but the runtime/backend selection story does not make that distinction clear.

**Why this matters**

This is mainly developer confusion. Misleading feature names create wrong assumptions about architecture and supported runtime behavior.

**Recommended direction**

Rename, document in code, or narrow the feature’s purpose so it matches behavior.

**Risk if changed carelessly**

Low.

---

### 14. Clone-heavy window transform helpers

**Priority:** P2

**What is wrong**

Some helper methods clone the full `Window` to apply small geometry updates.

**Why this matters**

This is not urgent now because `Window` is still small, but it is an avoidable inefficiency and a signal that geometry transforms could be cleaner.

**Recommended direction**

Either mutate in place where appropriate or introduce geometry-only helpers that return updated bounds rather than cloning the full model.

**Risk if changed carelessly**

Low.

---

### 15. Suspicious `Transform::Flipped180` default on nested output

**Priority:** P2

**What is wrong**

The nested output is initialized with `Transform::Flipped180`, which looks unusual.

**Why this matters**

This might be harmless, intentional, or a workaround. But if accidental, it is the kind of thing that silently skews coordinate assumptions.

**Recommended direction**

Verify intent. If it is a workaround, explain it in code. If not needed, remove it.

**Risk if changed carelessly**

Low.

---

## P3 — cleanup / code health

### 16. Geometry helpers are missing from core types

**Priority:** P3

**What is wrong**

`Rect` lacks common helpers like `contains`, `intersects`, edge helpers, and similar geometry conveniences.

**Why this matters**

This causes repeated ad-hoc geometry logic, which increases duplication and inconsistency risk.

**Recommended direction**

Add a small, disciplined geometry utility surface to `Rect` and related types.

---

### 17. `Vec2` operator surface is incomplete

**Priority:** P3

**What is wrong**

Useful operators like unary negation are missing.

**Why this matters**

Not a blocker, but it makes canvas math a bit noisier than it needs to be.

**Recommended direction**

Fill in the smallest useful operator set.

---

### 18. `WindowId` growth/overflow behavior should be explicit

**Priority:** P3

**What is wrong**

Headless code uses `wrapping_add`; live code increments directly. Overflow is not realistically imminent, but semantics are inconsistent.

**Why this matters**

ID generation policy should be intentional, even if “effectively infinite” in practice.

**Recommended direction**

Choose one explicit policy and encode it.

---

### 19. Heavy use of `eprintln!` instead of structured tracing

**Priority:** P3

**What is wrong**

The project initializes `tracing-subscriber` but still relies heavily on `eprintln!`.

**Why this matters**

This limits filtering, context enrichment, structured logs, and backend-specific diagnostics.

**Recommended direction**

Gradually move runtime diagnostics to `tracing` macros.

---

### 20. `flush_clients` timing is tied to redraw loop

**Priority:** P3

**What is wrong**

Client flushing currently happens as part of redraw flow.

**Why this matters**

That can hide latency dependencies where protocol-visible updates appear “because a redraw happened” rather than because the state change demanded a flush.

**Recommended direction**

Audit whether some flushes should happen closer to the actual event/state transition.

---

### 21. `compile_window_rules` duplicates config-to-runtime mapping logic

**Priority:** P3

**What is wrong**

Rule mapping is handwritten rather than expressed through a conversion layer.

**Why this matters**

Small issue, but it adds friction when rule fields evolve.

**Recommended direction**

Introduce a `From`/`TryFrom` conversion path.

---

### 22. Unsafe dispatch comment could be stronger

**Priority:** P3

**What is wrong**

The unsafe call around client dispatch has a safety comment, but it could be more explicit about exclusivity and thread assumptions.

**Why this matters**

Unsafe code deserves maximal clarity even when it is correct.

**Recommended direction**

Expand the safety note and tie it directly to the calloop/Wayland threading model.

---

## Suggested execution order

### Phase 1 — architecture and correctness

1. Unify output state model
2. Break up `EvilWm` into grouped sub-structs
3. Replace `with_live_lua` ownership workaround
4. Verify/fix SHM format advertisement
5. Stop unconditional redraws on nested backend

### Phase 2 — serious runtime quality

6. Decide real lock vs. renamed debug lock
7. Improve tty cursor handling
8. Add/verify tty damage tracking discipline
9. Reassess custom output-management protocol boundaries
10. Split probe support out of the main crate

### Phase 3 — polish and developer experience

11. Add PNG screenshot support
12. Improve hook error debug mode
13. Clarify x11 feature meaning
14. Clean up clone-heavy helpers
15. Verify/remove `Flipped180`

### Phase 4 — cleanup

16. Add geometry helpers
17. Expand `Vec2` ops
18. Make `WindowId` policy explicit
19. Migrate `eprintln!` to `tracing`
20. Audit flush timing
21. Deduplicate rule compilation
22. Strengthen unsafe docs

---

## Keep / drop guidance

If scope needs to be reduced, these are the strongest candidates to **keep** in the active plan:

- 1. Split output state model
- 2. `EvilWm` god struct refactor
- 3. `with_live_lua` ownership cleanup
- 4. SHM format verification/fix
- 5. Redraw scheduling cleanup
- 7. TTY cursor improvements
- 8. TTY damage tracking

If time is tight, the best candidates to **defer** are:

- 11. PPM screenshot format
- 13. x11 feature naming
- 14. Clone-heavy helpers
- 15. `Flipped180` verification
- 16–22. general cleanup items

---

## Checklist

### P0
- [x] Unify output state into one source of truth
- [x] Break `EvilWm` into grouped internal state structs
- [x] Replace `with_live_lua` take/reinsert ownership pattern
- [x] Verify and fix SHM format advertisement
- [x] Make redraw scheduling event-driven instead of effectively continuous

### P1
- [ ] Decide whether session lock is real security or a renamed debug/input shield
- [ ] Add better cursor handling for tty mode
- [ ] Add or verify damage-aware tty rendering
- [ ] Reduce maintenance burden of custom output-management protocol code
- [ ] Move probe/test client support into a separate crate or clearer boundary

### P2
- [ ] Add a practical screenshot format (PNG or similar)
- [ ] Add a verbose/developer mode for Lua hook error reporting
- [ ] Clarify or rename x11-related feature/runtime behavior
- [ ] Remove unnecessary full-window cloning in geometry helpers
- [ ] Verify whether `Transform::Flipped180` is intentional

### P3
- [ ] Add missing `Rect` geometry helpers
- [ ] Expand `Vec2` operator helpers
- [ ] Make `WindowId` generation semantics explicit
- [ ] Migrate diagnostics from `eprintln!` to `tracing`
- [ ] Audit whether client flushing should happen outside redraw-only flow
- [ ] Replace handwritten rule compilation with a conversion layer
- [ ] Strengthen unsafe safety documentation around client dispatch
