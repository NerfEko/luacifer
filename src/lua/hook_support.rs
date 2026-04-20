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
            PropertyValue::OptionString(Some(s)) => {
                lua.create_string(s.as_str()).map(Value::String)
            }
            PropertyValue::OptionString(None) => Ok(Value::Nil),
            PropertyValue::Bool(b) => Ok(Value::Boolean(*b)),
        }
    }

    pub(crate) fn to_json_value(&self) -> serde_json::Value {
        match self {
            PropertyValue::OptionString(Some(s)) => serde_json::Value::String(s.clone()),
            PropertyValue::OptionString(None) => serde_json::Value::Null,
            PropertyValue::Bool(b) => serde_json::Value::Bool(*b),
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
    state.outputs.iter().find(|output| output.id == id).cloned()
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
        context.set("pointer", pointer_to_table(lua, state, pointer)?)?;
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
        context.set("pointer", pointer_to_table(lua, params.state, pointer)?)?;
    }

    if let Some(button) = params.button {
        context.set("button", button)?;
        context.set("button_name", button_name(button))?;
        context.set("button_info", button_to_table(lua, button)?)?;
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
    context.set("bound_action", bound_action)?;
    context.set("action", bound_action)?;
    context.set("has_binding", bound_action.is_some())?;
    context.set(
        "pointer",
        pointer_to_table(lua, state, Point::new(state.pointer.x, state.pointer.y))?,
    )?;
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
    viewport.set("world_x", output.viewport.x)?;
    viewport.set("world_y", output.viewport.y)?;
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

fn modifier_names(modifiers: ModifierSet) -> Vec<&'static str> {
    let mut names = Vec::new();
    if modifiers.ctrl {
        names.push("Ctrl");
    }
    if modifiers.alt {
        names.push("Alt");
    }
    if modifiers.shift {
        names.push("Shift");
    }
    if modifiers.logo {
        names.push("Super");
    }
    names
}

fn button_name(button: u32) -> Option<&'static str> {
    match button {
        272 => Some("left"),
        273 => Some("right"),
        274 => Some("middle"),
        _ => None,
    }
}

fn button_to_table(lua: &Lua, button: u32) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("code", button)?;
    table.set("name", button_name(button))?;
    table.set("left", button == 272)?;
    table.set("right", button == 273)?;
    table.set("middle", button == 274)?;
    table.set("known", button_name(button).is_some())?;
    Ok(table)
}

fn pointer_to_table(
    lua: &Lua,
    state: &RuntimeStateSnapshot,
    pointer: Point,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set("x", pointer.x)?;
    table.set("y", pointer.y)?;

    let output = find_output_snapshot_at_point(state, pointer);
    table.set(
        "output_id",
        output.as_ref().map(|output| output.id.as_str()),
    )?;
    if let Some(output) = output {
        table.set("local_x", pointer.x - output.logical_x)?;
        table.set("local_y", pointer.y - output.logical_y)?;
    } else {
        table.set("local_x", Value::Nil)?;
        table.set("local_y", Value::Nil)?;
    }

    Ok(table)
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
    table.set("logo", modifiers.logo)?;

    let names = modifier_names(modifiers);
    let names_table = lua.create_table()?;
    for (index, name) in names.iter().enumerate() {
        names_table.set(index + 1, *name)?;
    }
    table.set("names", names_table)?;
    table.set("count", names.len())?;
    table.set("any", !names.is_empty())?;
    table.set("none", names.is_empty())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::{OutputSnapshot, PointerSnapshot, ViewportSnapshot};

    fn sample_state() -> RuntimeStateSnapshot {
        RuntimeStateSnapshot {
            focused_window_id: Some(7),
            pointer: PointerSnapshot { x: 320.0, y: 180.0 },
            outputs: vec![OutputSnapshot {
                id: "nested".into(),
                logical_x: 0.0,
                logical_y: 0.0,
                viewport: ViewportSnapshot {
                    x: 0.0,
                    y: 0.0,
                    zoom: 1.25,
                    screen_w: 1280.0,
                    screen_h: 720.0,
                    visible_world: Rect::new(0.0, 0.0, 1024.0, 576.0),
                },
            }],
            windows: vec![WindowSnapshot {
                id: 7,
                app_id: Some("foot".into()),
                title: Some("shell".into()),
                bounds: Rect::new(100.0, 200.0, 640.0, 480.0),
                floating: false,
                exclude_from_focus: false,
                focused: true,
                fullscreen: false,
                maximized: false,
                urgent: false,
                mapped: true,
                mapped_at: Some(123.0),
                last_focused_at: Some(456.0),
                output_id: Some("nested".into()),
                pid: Some(999),
            }],
        }
    }

    #[test]
    fn focus_hook_context_exposes_aliases_and_state() {
        let lua = Lua::new();
        let state = sample_state();
        let window = &state.windows[0];
        let ctx = focus_hook_context(&lua, &state, None, Some(window)).expect("focus context");

        assert_eq!(
            ctx.get::<Option<u64>>("previous_window_id")
                .expect("previous"),
            None
        );
        assert_eq!(
            ctx.get::<Option<u64>>("focused_window_id")
                .expect("focused"),
            Some(7)
        );
        assert!(ctx.get::<Table>("state").is_ok(), "state table must exist");
        assert!(
            ctx.get::<Table>("focused_window").is_ok(),
            "focused_window alias must exist"
        );
    }

    #[test]
    fn property_changed_context_exposes_window_id_and_values() {
        let lua = Lua::new();
        let state = sample_state();
        let window = &state.windows[0];
        let ctx = property_changed_hook_context(
            &lua,
            &state,
            window,
            "title",
            &PropertyValue::OptionString(None),
            &PropertyValue::OptionString(Some("shell".into())),
        )
        .expect("property context");

        assert_eq!(ctx.get::<u64>("window_id").expect("window_id"), 7);
        assert_eq!(ctx.get::<String>("property").expect("property"), "title");
        assert!(
            ctx.get::<Table>("window").is_ok(),
            "window alias must exist"
        );
        assert!(ctx.get::<Table>("state").is_ok(), "state alias must exist");
        assert!(ctx.get::<Value>("old_value").expect("old").is_nil());
        let Value::String(new_value) = ctx.get::<Value>("new_value").expect("new") else {
            panic!("expected string new value")
        };
        assert_eq!(new_value.to_str().expect("utf8 new value"), "shell");
    }

    #[test]
    fn key_hook_context_exposes_binding_pointer_and_modifier_metadata() {
        let lua = Lua::new();
        let state = sample_state();
        let ctx = key_hook_context(
            &lua,
            &state,
            "Ctrl+Shift+K",
            "K",
            ModifierSet {
                ctrl: true,
                alt: false,
                shift: true,
                logo: true,
            },
            Some("pan_left"),
        )
        .expect("key context");

        assert_eq!(
            ctx.get::<String>("keyspec").expect("keyspec"),
            "Ctrl+Shift+K"
        );
        assert_eq!(ctx.get::<String>("key").expect("key"), "K");
        assert_eq!(
            ctx.get::<String>("bound_action").expect("bound_action"),
            "pan_left"
        );
        assert_eq!(ctx.get::<String>("action").expect("action"), "pan_left");
        assert!(ctx.get::<bool>("has_binding").expect("has_binding"));

        let pointer = ctx.get::<Table>("pointer").expect("pointer");
        assert_eq!(
            pointer.get::<String>("output_id").expect("output_id"),
            "nested"
        );
        assert_eq!(pointer.get::<f64>("local_x").expect("local_x"), 320.0);
        assert_eq!(pointer.get::<f64>("local_y").expect("local_y"), 180.0);

        let modifiers = ctx.get::<Table>("modifiers").expect("modifiers");
        assert!(modifiers.get::<bool>("ctrl").expect("ctrl"));
        assert!(modifiers.get::<bool>("shift").expect("shift"));
        assert!(modifiers.get::<bool>("super").expect("super"));
        assert!(modifiers.get::<bool>("logo").expect("logo"));
        assert!(modifiers.get::<bool>("any").expect("any"));
        assert!(!modifiers.get::<bool>("none").expect("none"));
        assert_eq!(modifiers.get::<usize>("count").expect("count"), 3);
        let names = modifiers.get::<Table>("names").expect("names");
        assert_eq!(names.get::<String>(1).expect("name 1"), "Ctrl");
        assert_eq!(names.get::<String>(2).expect("name 2"), "Shift");
        assert_eq!(names.get::<String>(3).expect("name 3"), "Super");
    }

    #[test]
    fn resolve_focus_context_exposes_window_previous_and_modifiers() {
        let lua = Lua::new();
        let state = sample_state();
        let window = &state.windows[0];
        let ctx = focus_resolve_context(
            &lua,
            ResolveFocusContext {
                reason: "pointer_button",
                state: &state,
                window: Some(window),
                previous: Some(window),
                pointer: Some(Point::new(12.0, 34.0)),
                button: Some(272),
                pressed: Some(true),
                modifiers: Some(ModifierSet {
                    ctrl: true,
                    alt: false,
                    shift: true,
                    logo: false,
                }),
            },
        )
        .expect("resolve context");

        assert_eq!(
            ctx.get::<String>("reason").expect("reason"),
            "pointer_button"
        );
        assert_eq!(ctx.get::<u64>("window_id").expect("window id"), 7);
        assert_eq!(
            ctx.get::<u64>("previous_window_id").expect("previous id"),
            7
        );
        assert_eq!(ctx.get::<u32>("button").expect("button"), 272);
        assert_eq!(
            ctx.get::<String>("button_name").expect("button_name"),
            "left"
        );
        let button_info = ctx.get::<Table>("button_info").expect("button_info");
        assert_eq!(button_info.get::<u32>("code").expect("code"), 272);
        assert!(button_info.get::<bool>("left").expect("left"));
        assert!(button_info.get::<bool>("known").expect("known"));
        assert!(ctx.get::<bool>("pressed").expect("pressed"));
        let pointer = ctx.get::<Table>("pointer").expect("pointer");
        assert_eq!(pointer.get::<f64>("x").expect("pointer x"), 12.0);
        assert_eq!(
            pointer.get::<String>("output_id").expect("output_id"),
            "nested"
        );
        assert_eq!(pointer.get::<f64>("local_x").expect("local_x"), 12.0);
        assert_eq!(pointer.get::<f64>("local_y").expect("local_y"), 34.0);
        let modifiers = ctx.get::<Table>("modifiers").expect("modifiers");
        assert!(modifiers.get::<bool>("ctrl").expect("ctrl"));
        assert!(modifiers.get::<bool>("shift").expect("shift"));
        assert!(modifiers.get::<bool>("any").expect("any"));
        assert_eq!(modifiers.get::<usize>("count").expect("count"), 2);
    }
}
