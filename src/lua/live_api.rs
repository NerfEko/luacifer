use std::{cell::RefCell, rc::Rc};

use mlua::{Lua, Table, Value};

use crate::{
    canvas::Point,
    lua::{
        ConfigError, HookAction, RuntimeStateSnapshot,
        hook_support::{
            find_output_snapshot, find_output_snapshot_at_point, find_primary_output_snapshot,
            output_to_table, rect_to_table, snapshot_to_table, window_to_table,
        },
        register_draw_api,
    },
    window::ResizeEdges,
};

pub(super) fn register_live_api(
    lua: &Lua,
    action_queue: &Rc<RefCell<Vec<HookAction>>>,
    query_snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
    allow_setup_calls_during_load: &Rc<RefCell<bool>>,
) -> Result<(), ConfigError> {
    let evil = lua.create_table()?;

    register_setup_placeholders(&evil, lua, allow_setup_calls_during_load)?;
    register_query_helpers(&evil, lua, query_snapshot)?;
    register_window_api(&evil, lua, action_queue, query_snapshot)?;
    register_canvas_api(&evil, lua, action_queue, query_snapshot)?;

    evil.set("on", lua.create_table()?)?;
    register_draw_api(lua, &evil)?;
    lua.globals().set("evil", evil)?;
    Ok(())
}

fn register_setup_placeholders(
    evil: &Table,
    lua: &Lua,
    allow_setup_calls_during_load: &Rc<RefCell<bool>>,
) -> Result<(), ConfigError> {
    let allow_config = allow_setup_calls_during_load.clone();
    let config = lua.create_function(move |_, _: Table| -> mlua::Result<()> {
        if *allow_config.borrow() {
            return Ok(());
        }
        Err(config_time_only_error("evil.config"))
    })?;
    evil.set("config", config)?;

    let allow_bind = allow_setup_calls_during_load.clone();
    let bind = lua.create_function(move |_, _: Value| -> mlua::Result<()> {
        if *allow_bind.borrow() {
            return Ok(());
        }
        Err(config_time_only_error("evil.bind"))
    })?;
    evil.set("bind", bind)?;

    let allow_autostart = allow_setup_calls_during_load.clone();
    let autostart = lua.create_function(move |_, _: String| -> mlua::Result<()> {
        if *allow_autostart.borrow() {
            return Ok(());
        }
        Err(config_time_only_error("evil.autostart"))
    })?;
    evil.set("autostart", autostart)?;

    Ok(())
}

fn register_query_helpers(
    evil: &Table,
    lua: &Lua,
    query_snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
) -> Result<(), ConfigError> {
    let state_snapshot = query_snapshot.clone();
    let state = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&state_snapshot)?;
        snapshot_to_table(lua, &snapshot)
    })?;
    evil.set("state", state)?;

    let pointer = lua.create_table()?;
    let pointer_snapshot = query_snapshot.clone();
    let position = lua.create_function(move |lua, ()| {
        let pointer = current_live_snapshot(&pointer_snapshot)?.pointer;
        let table = lua.create_table()?;
        table.set("x", pointer.x)?;
        table.set("y", pointer.y)?;
        Ok(table)
    })?;
    pointer.set("position", position)?;
    evil.set("pointer", pointer)?;

    let output = lua.create_table()?;
    let output_snapshot = query_snapshot.clone();
    let list_outputs = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&output_snapshot)?;
        let outputs = lua.create_table()?;
        for (index, output) in snapshot.outputs.iter().enumerate() {
            outputs.set(index + 1, output_to_table(lua, output)?)?;
        }
        Ok(outputs)
    })?;
    output.set("list", list_outputs)?;

    let get_output_snapshot = query_snapshot.clone();
    let get_output = lua.create_function(move |lua, id: String| {
        let snapshot = current_live_snapshot(&get_output_snapshot)?;
        find_output_snapshot(&snapshot, &id)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output.set("get", get_output)?;

    let primary_output_snapshot = query_snapshot.clone();
    let primary_output = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&primary_output_snapshot)?;
        find_primary_output_snapshot(&snapshot)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output.set("primary", primary_output)?;

    let pointer_output_snapshot = query_snapshot.clone();
    let output_at_pointer = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&pointer_output_snapshot)?;
        find_output_snapshot_at_point(
            &snapshot,
            Point::new(snapshot.pointer.x, snapshot.pointer.y),
        )
        .map(|output| output_to_table(lua, &output))
        .transpose()
    })?;
    output.set("at_pointer", output_at_pointer)?;
    evil.set("output", output)?;

    Ok(())
}

fn register_window_api(
    evil: &Table,
    lua: &Lua,
    action_queue: &Rc<RefCell<Vec<HookAction>>>,
    query_snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
) -> Result<(), ConfigError> {
    let window = lua.create_table()?;

    let move_queue = action_queue.clone();
    let move_window = lua.create_function(move |_, (id, x, y): (u64, f64, f64)| {
        move_queue
            .borrow_mut()
            .push(HookAction::MoveWindow { id, x, y });
        Ok(true)
    })?;
    window.set("move", move_window)?;

    let resize_queue = action_queue.clone();
    let resize = lua.create_function(move |_, (id, w, h): (u64, f64, f64)| {
        resize_queue
            .borrow_mut()
            .push(HookAction::ResizeWindow { id, w, h });
        Ok(true)
    })?;
    window.set("resize", resize)?;

    let set_bounds_queue = action_queue.clone();
    let set_bounds =
        lua.create_function(move |_, (id, x, y, w, h): (u64, f64, f64, f64, f64)| {
            set_bounds_queue
                .borrow_mut()
                .push(HookAction::SetBounds { id, x, y, w, h });
            Ok(true)
        })?;
    window.set("set_bounds", set_bounds)?;

    let begin_move_queue = action_queue.clone();
    let begin_move = lua.create_function(move |_, id: u64| {
        begin_move_queue
            .borrow_mut()
            .push(HookAction::BeginInteractiveMove { id });
        Ok(true)
    })?;
    window.set("begin_move", begin_move)?;

    let begin_resize_queue = action_queue.clone();
    let begin_resize = lua.create_function(move |_, (id, edges): (u64, Table)| {
        let edges = ResizeEdges {
            left: edges.get::<Option<bool>>("left")?.unwrap_or(false),
            right: edges.get::<Option<bool>>("right")?.unwrap_or(false),
            top: edges.get::<Option<bool>>("top")?.unwrap_or(false),
            bottom: edges.get::<Option<bool>>("bottom")?.unwrap_or(false),
        };
        if !(edges.left || edges.right || edges.top || edges.bottom) {
            return Ok(false);
        }
        begin_resize_queue
            .borrow_mut()
            .push(HookAction::BeginInteractiveResize { id, edges });
        Ok(true)
    })?;
    window.set("begin_resize", begin_resize)?;

    let focus_queue = action_queue.clone();
    let focus = lua.create_function(move |_, id: u64| {
        focus_queue
            .borrow_mut()
            .push(HookAction::FocusWindow { id });
        Ok(true)
    })?;
    window.set("focus", focus)?;

    let clear_focus_queue = action_queue.clone();
    let clear_focus = lua.create_function(move |_, ()| {
        clear_focus_queue.borrow_mut().push(HookAction::ClearFocus);
        Ok(true)
    })?;
    window.set("clear_focus", clear_focus)?;

    let close_queue = action_queue.clone();
    let close = lua.create_function(move |_, id: u64| {
        close_queue
            .borrow_mut()
            .push(HookAction::CloseWindow { id });
        Ok(true)
    })?;
    window.set("close", close)?;

    let list_snapshot = query_snapshot.clone();
    let list = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&list_snapshot)?;
        let windows = lua.create_table()?;
        for (index, window) in snapshot.windows.iter().enumerate() {
            windows.set(index + 1, window_to_table(lua, window)?)?;
        }
        Ok(windows)
    })?;
    window.set("list", list)?;

    let get_snapshot = query_snapshot.clone();
    let get = lua.create_function(move |lua, id: u64| {
        let snapshot = current_live_snapshot(&get_snapshot)?;
        snapshot
            .windows
            .iter()
            .find(|window| window.id == id)
            .map(|window| window_to_table(lua, window))
            .transpose()
    })?;
    window.set("get", get)?;

    let focused_snapshot = query_snapshot.clone();
    let focused = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&focused_snapshot)?;
        snapshot
            .focused_window_id
            .and_then(|id| snapshot.windows.iter().find(|window| window.id == id))
            .map(|window| window_to_table(lua, window))
            .transpose()
    })?;
    window.set("focused", focused)?;

    evil.set("window", window)?;
    Ok(())
}

fn register_canvas_api(
    evil: &Table,
    lua: &Lua,
    action_queue: &Rc<RefCell<Vec<HookAction>>>,
    query_snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
) -> Result<(), ConfigError> {
    let canvas = lua.create_table()?;

    let pan_queue = action_queue.clone();
    let pan = lua.create_function(move |_, (dx, dy): (f64, f64)| {
        pan_queue
            .borrow_mut()
            .push(HookAction::PanCanvas { dx, dy });
        Ok(true)
    })?;
    canvas.set("pan", pan)?;

    let zoom_queue = action_queue.clone();
    let zoom = lua.create_function(move |_, factor: f64| {
        zoom_queue
            .borrow_mut()
            .push(HookAction::ZoomCanvas { factor });
        Ok(true)
    })?;
    canvas.set("zoom", zoom)?;

    let viewport_snapshot = query_snapshot.clone();
    let viewport = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&viewport_snapshot)?;
        let Some(output) = snapshot.outputs.first() else {
            return lua.create_table();
        };
        let table = lua.create_table()?;
        table.set("x", output.viewport.x)?;
        table.set("y", output.viewport.y)?;
        table.set("world_x", output.viewport.x)?;
        table.set("world_y", output.viewport.y)?;
        table.set("zoom", output.viewport.zoom)?;
        table.set("screen_w", output.viewport.screen_w)?;
        table.set("screen_h", output.viewport.screen_h)?;
        table.set(
            "visible_world",
            rect_to_table(lua, output.viewport.visible_world)?,
        )?;
        Ok(table)
    })?;
    canvas.set("viewport", viewport)?;

    evil.set("canvas", canvas)?;
    Ok(())
}

fn current_live_snapshot(
    snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
) -> mlua::Result<RuntimeStateSnapshot> {
    snapshot.borrow().clone().ok_or_else(|| {
        mlua::Error::runtime(
            "live query helpers are only available while a hook is running; use ctx.state inside hooks",
        )
    })
}

fn config_time_only_error(function_name: &str) -> mlua::Error {
    mlua::Error::runtime(format!(
        "{function_name}() is config-time only and unavailable in the live hook runtime"
    ))
}
