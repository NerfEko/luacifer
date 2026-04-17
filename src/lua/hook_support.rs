use mlua::{Lua, Table, Value};

use crate::{
    canvas::{Point, Rect, Vec2},
    input::ModifierSet,
    lua::{OutputSnapshot, RuntimeStateSnapshot, WindowSnapshot},
    window::{ResizeEdges, WindowId},
};

/// A typed property value passed to the `window_property_changed` hook context.
///
/// Covers the initial supported property types (string-valued and bool-valued window properties).
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// An optional string (covers `title`, `app_id`). `None` maps to Lua `nil`.
    OptionString(Option<String>),
    /// A boolean (covers `floating`, `exclude_from_focus`, etc.).
    Bool(bool),
}

impl PropertyValue {
    fn to_lua_value(&self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            PropertyValue::OptionString(Some(s)) => lua.create_string(s.as_str()).map(Value::String),
            PropertyValue::OptionString(None) => Ok(Value::Nil),
            PropertyValue::Bool(b) => Ok(Value::Boolean(*b)),
        }
    }
}

pub struct ResolveFocusContext<'a> {
    pub reason: &'a str,
    pub state: &'a RuntimeStateSnapshot,
    pub window: Option<&'a WindowSnapshot>,
    pub previous: Option<&'a WindowSnapshot>,
    pub pointer: Option<Point>,
    pub button: Option<u32>,
    pub pressed: Option<bool>,
    pub modifiers: Option<ModifierSet>,
}

pub fn find_window_snapshot(state: &RuntimeStateSnapshot, id: WindowId) -> Option<WindowSnapshot> {
    state
        .windows
        .iter()
        .find(|window| window.id == id.0)
        .cloned()
}

pub fn find_output_snapshot(state: &RuntimeStateSnapshot, id: &str) -> Option<OutputSnapshot> {
    state
        .outputs
        .iter()
        .find(|output| output.id == id)
        .cloned()
}

pub fn find_primary_output_snapshot(state: &RuntimeStateSnapshot) -> Option<OutputSnapshot> {
    state.outputs.first().cloned()
}

pub fn find_output_snapshot_at_point(
    state: &RuntimeStateSnapshot,
    point: Point,
) -> Option<OutputSnapshot> {
    state
        .outputs
        .iter()
        .find(|output| {
            point.x >= output.logical_x
                && point.x < output.logical_x + output.viewport.screen_w
                && point.y >= output.logical_y
                && point.y < output.logical_y + output.viewport.screen_h
        })
        .cloned()
}

pub fn base_hook_context(
    lua: &Lua,
    event: &str,
    state: &RuntimeStateSnapshot,
) -> mlua::Result<Table> {
    let context = lua.create_table()?;
    context.set("event", event)?;
    context.set("state", snapshot_to_table(lua, state)?)?;
    Ok(context)
}

pub fn window_hook_context(
    lua: &Lua,
    event: &str,
    state: &RuntimeStateSnapshot,
    window: &WindowSnapshot,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, event, state)?;
    context.set("window", window_to_table(lua, window)?)?;
    context.set("window_id", window.id)?;
    Ok(context)
}

pub fn delta_hook_context(
    lua: &Lua,
    event: &str,
    state: &RuntimeStateSnapshot,
    window: &WindowSnapshot,
    delta: Vec2,
    pointer: Option<Point>,
    edges: Option<ResizeEdges>,
) -> mlua::Result<Table> {
    let context = window_hook_context(lua, event, state, window)?;

    let delta_table = lua.create_table()?;
    delta_table.set("x", delta.x)?;
    delta_table.set("y", delta.y)?;
    context.set("delta", delta_table)?;
    context.set("dx", delta.x)?;
    context.set("dy", delta.y)?;

    if let Some(pointer) = pointer {
        let pointer_table = lua.create_table()?;
        pointer_table.set("x", pointer.x)?;
        pointer_table.set("y", pointer.y)?;
        context.set("pointer", pointer_table)?;
    }

    if let Some(edges) = edges {
        context.set("edges", resize_edges_to_table(lua, edges)?)?;
    }

    Ok(context)
}

pub fn focus_hook_context(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    previous: Option<&WindowSnapshot>,
    current: Option<&WindowSnapshot>,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "focus_changed", state)?;
    context.set("previous_window_id", previous.map(|window| window.id))?;
    context.set("focused_window_id", current.map(|window| window.id))?;

    if let Some(previous) = previous {
        context.set("previous_window", window_to_table(lua, previous)?)?;
    }
    if let Some(current) = current {
        context.set("focused_window", window_to_table(lua, current)?)?;
    }

    Ok(context)
}

pub fn focus_resolve_context(lua: &Lua, params: ResolveFocusContext<'_>) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "resolve_focus", params.state)?;
    context.set("reason", params.reason)?;

    if let Some(window) = params.window {
        context.set("window", window_to_table(lua, window)?)?;
        context.set("window_id", window.id)?;
    }

    if let Some(previous) = params.previous {
        context.set("previous_window", window_to_table(lua, previous)?)?;
        context.set("previous_window_id", previous.id)?;
    }

    let focused = params
        .state
        .focused_window_id
        .and_then(|id| params.state.windows.iter().find(|window| window.id == id));
    context.set("focused_window_id", params.state.focused_window_id)?;
    if let Some(focused) = focused {
        context.set("focused_window", window_to_table(lua, focused)?)?;
    }

    if let Some(pointer) = params.pointer {
        let pointer_table = lua.create_table()?;
        pointer_table.set("x", pointer.x)?;
        pointer_table.set("y", pointer.y)?;
        context.set("pointer", pointer_table)?;
    }

    if let Some(button) = params.button {
        context.set("button", button)?;
    }
    if let Some(pressed) = params.pressed {
        context.set("pressed", pressed)?;
    }
    if let Some(modifiers) = params.modifiers {
        context.set("modifiers", modifiers_to_table(lua, modifiers)?)?;
    }

    Ok(context)
}

pub fn draw_hook_context(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    output: &OutputSnapshot,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "draw", state)?;
    let output_table = output_to_table(lua, output)?;
    let viewport = output_table.get::<Table>("viewport")?;
    context.set("output", output_table)?;
    context.set("viewport", viewport)?;
    context.set("focused_window_id", state.focused_window_id)?;
    if let Some(focused) = state
        .focused_window_id
        .and_then(|id| state.windows.iter().find(|window| window.id == id))
    {
        context.set("focused_window", window_to_table(lua, focused)?)?;
    }
    Ok(context)
}

pub fn key_hook_context(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    keyspec: &str,
    key: &str,
    modifiers: ModifierSet,
    bound_action: Option<&str>,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "key", state)?;
    context.set("keyspec", keyspec)?;
    context.set("key", key)?;
    context.set("modifiers", modifiers_to_table(lua, modifiers)?)?;
    if let Some(bound_action) = bound_action {
        context.set("bound_action", bound_action)?;
    }
    Ok(context)
}

pub fn gesture_hook_context(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    kind: &str,
    fingers: u32,
    delta: Vec2,
    scale: Option<f64>,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "gesture", state)?;
    context.set("kind", kind)?;
    context.set("fingers", fingers)?;
    context.set("dx", delta.x)?;
    context.set("dy", delta.y)?;

    let delta_table = lua.create_table()?;
    delta_table.set("x", delta.x)?;
    delta_table.set("y", delta.y)?;
    context.set("delta", delta_table)?;

    if let Some(scale) = scale {
        context.set("scale", scale)?;
    }

    Ok(context)
}

pub fn snapshot_to_table(lua: &Lua, snapshot: &RuntimeStateSnapshot) -> mlua::Result<Table> {
    let state = lua.create_table()?;
    state.set("focused_window_id", snapshot.focused_window_id)?;

    let pointer = lua.create_table()?;
    pointer.set("x", snapshot.pointer.x)?;
    pointer.set("y", snapshot.pointer.y)?;
    state.set("pointer", pointer)?;

    let outputs = lua.create_table()?;
    for (index, output) in snapshot.outputs.iter().enumerate() {
        outputs.set(index + 1, output_to_table(lua, output)?)?;
    }
    state.set("outputs", outputs)?;

    let windows = lua.create_table()?;
    for (index, window) in snapshot.windows.iter().enumerate() {
        windows.set(index + 1, window_to_table(lua, window)?)?;
    }
    state.set("windows", windows)?;

    Ok(state)
}

pub fn output_to_table(lua: &Lua, output: &OutputSnapshot) -> mlua::Result<Table> {
    let output_table = lua.create_table()?;
    output_table.set("id", output.id.as_str())?;
    output_table.set("logical_x", output.logical_x)?;
    output_table.set("logical_y", output.logical_y)?;

    let viewport = lua.create_table()?;
    viewport.set("x", output.viewport.x)?;
    viewport.set("y", output.viewport.y)?;
    viewport.set("zoom", output.viewport.zoom)?;
    viewport.set("screen_w", output.viewport.screen_w)?;
    viewport.set("screen_h", output.viewport.screen_h)?;
    viewport.set(
        "visible_world",
        rect_to_table(lua, output.viewport.visible_world)?,
    )?;
    output_table.set("viewport", viewport)?;

    let logical_bounds = Rect::new(
        output.logical_x,
        output.logical_y,
        output.viewport.screen_w,
        output.viewport.screen_h,
    );
    let screen_bounds = Rect::new(0.0, 0.0, output.viewport.screen_w, output.viewport.screen_h);
    output_table.set("bounds", rect_to_table(lua, logical_bounds)?)?;
    output_table.set("logical_bounds", rect_to_table(lua, logical_bounds)?)?;
    output_table.set("screen_bounds", rect_to_table(lua, screen_bounds)?)?;
    output_table.set(
        "visible_world",
        rect_to_table(lua, output.viewport.visible_world)?,
    )?;

    Ok(output_table)
}

pub fn window_to_table(lua: &Lua, window: &WindowSnapshot) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("id", window.id)?;
    table.set("app_id", window.app_id.as_deref())?;
    table.set("title", window.title.as_deref())?;
    table.set("x", window.bounds.origin.x)?;
    table.set("y", window.bounds.origin.y)?;
    table.set("w", window.bounds.size.w)?;
    table.set("h", window.bounds.size.h)?;
    table.set("bounds", rect_to_table(lua, window.bounds)?)?;
    table.set("floating", window.floating)?;
    table.set("exclude_from_focus", window.exclude_from_focus)?;
    table.set("focused", window.focused)?;
    // Phase 1A additions
    table.set("fullscreen", window.fullscreen)?;
    table.set("maximized", window.maximized)?;
    table.set("urgent", window.urgent)?;
    table.set("mapped", window.mapped)?;
    table.set("mapped_at", window.mapped_at)?;
    table.set("last_focused_at", window.last_focused_at)?;
    table.set("output_id", window.output_id.as_deref())?;
    table.set("pid", window.pid)?;
    Ok(table)
}

pub fn modifiers_to_table(lua: &Lua, modifiers: ModifierSet) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("ctrl", modifiers.ctrl)?;
    table.set("alt", modifiers.alt)?;
    table.set("shift", modifiers.shift)?;
    table.set("super", modifiers.logo)?;
    Ok(table)
}

pub fn resize_edges_to_table(lua: &Lua, edges: ResizeEdges) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("left", edges.left)?;
    table.set("right", edges.right)?;
    table.set("top", edges.top)?;
    table.set("bottom", edges.bottom)?;
    Ok(table)
}

pub fn rect_to_table(lua: &Lua, rect: Rect) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", rect.origin.x)?;
    table.set("y", rect.origin.y)?;
    table.set("w", rect.size.w)?;
    table.set("h", rect.size.h)?;
    Ok(table)
}

/// Build the hook context table for `evil.on.window_property_changed`.
///
/// `ctx.window` reflects the **new** state after the property changed.
/// `ctx.old_value` and `ctx.new_value` carry the before/after values explicitly.
pub fn property_changed_hook_context(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    window: &WindowSnapshot,
    property: &str,
    old_value: &PropertyValue,
    new_value: &PropertyValue,
) -> mlua::Result<Table> {
    let context = base_hook_context(lua, "window_property_changed", state)?;
    context.set("window", window_to_table(lua, window)?)?;
    context.set("window_id", window.id)?;
    context.set("property", property)?;
    context.set("old_value", old_value.to_lua_value(lua)?)?;
    context.set("new_value", new_value.to_lua_value(lua)?)?;
    Ok(context)
}
