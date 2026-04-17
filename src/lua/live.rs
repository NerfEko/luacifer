use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use mlua::{Function, Lua, Table, Value};

use crate::{
    canvas::{Point, Vec2},
    compositor::EvilWm,
    input::{ModifierSet, parse_keyspec},
    lua::{
        ConfigError, DrawCommand, HookAction, OutputSnapshot, PropertyValue, RuntimeStateSnapshot,
        WindowSnapshot, apply_hook_action, config::resolve_include_path, parse_draw_commands,
        register_draw_api,
        hook_support::{
            ResolveFocusContext, delta_hook_context, draw_hook_context, find_output_snapshot,
            find_output_snapshot_at_point, find_primary_output_snapshot, find_window_snapshot,
            focus_hook_context, focus_resolve_context, gesture_hook_context, key_hook_context,
            output_to_table, property_changed_hook_context, rect_to_table, snapshot_to_table,
            window_hook_context, window_to_table,
        },
        parse_hook_actions,
    },
    window::{ResizeEdges, WindowId},
};

pub struct ResolveFocusRequest<'a> {
    pub reason: &'a str,
    pub window: Option<&'a WindowSnapshot>,
    pub previous: Option<WindowId>,
    pub pointer: Option<Point>,
    pub button: Option<u32>,
    pub pressed: Option<bool>,
    pub modifiers: Option<ModifierSet>,
}

#[derive(Debug)]
pub struct LiveLuaHooks {
    lua: Lua,
    action_queue: Rc<RefCell<Vec<HookAction>>>,
    query_snapshot: Rc<RefCell<Option<RuntimeStateSnapshot>>>,
    allow_setup_calls_during_load: Rc<RefCell<bool>>,
}

impl LiveLuaHooks {
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let lua = Lua::new();
        let base_dir = base_dir.into();

        let action_queue = Rc::new(RefCell::new(Vec::new()));
        let query_snapshot = Rc::new(RefCell::new(None));
        let allow_setup_calls_during_load = Rc::new(RefCell::new(false));

        register_include(&lua, base_dir.clone())?;
        register_live_api(
            &lua,
            &action_queue,
            &query_snapshot,
            &allow_setup_calls_during_load,
        )?;

        Ok(Self {
            lua,
            action_queue,
            query_snapshot,
            allow_setup_calls_during_load,
        })
    }

    pub fn load_script_file(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        let source = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        self.load_script_str(&source, path)
    }

    pub fn load_script_str(&self, source: &str, name: impl AsRef<Path>) -> Result<(), ConfigError> {
        let name = name.as_ref();
        *self.allow_setup_calls_during_load.borrow_mut() = true;
        let result = self
            .lua
            .load(source)
            .set_name(name.to_string_lossy().as_ref())
            .eval::<Value>()
            .map(|_| ())
            .map_err(ConfigError::from);
        *self.allow_setup_calls_during_load.borrow_mut() = false;
        result
    }

    pub fn has_hook(&self, hook_name: &str) -> Result<bool, ConfigError> {
        Ok(self.lookup_hook(hook_name)?.is_some())
    }

    pub fn draw_commands_for_output(
        &self,
        state: &mut EvilWm,
        hook_name: &str,
        output_snapshot: &OutputSnapshot,
    ) -> Result<Vec<DrawCommand>, ConfigError> {
        let snapshot = state.state_snapshot();
        let context = draw_hook_context(&self.lua, &snapshot, output_snapshot)?;
        self.call_draw_hook(state, &snapshot, hook_name, context)
    }

    #[cfg(test)]
    fn eval_for_tests(&self, source: &str, name: &str) -> Result<Value, ConfigError> {
        self.lua
            .load(source)
            .set_name(name)
            .eval::<Value>()
            .map_err(ConfigError::from)
    }

    #[cfg(test)]
    fn set_query_snapshot_for_tests(&self, snapshot: Option<RuntimeStateSnapshot>) {
        self.query_snapshot.replace(snapshot);
    }

    pub fn trigger_place_window(
        &self,
        state: &mut EvilWm,
        id: WindowId,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "place_window",
            window_hook_context(&self.lua, "place_window", &snapshot, &window)?,
        )
    }

    pub fn trigger_window_mapped(
        &self,
        state: &mut EvilWm,
        id: WindowId,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "window_mapped",
            window_hook_context(&self.lua, "window_mapped", &snapshot, &window)?,
        )
    }

    pub fn trigger_window_unmapped(
        &self,
        state: &mut EvilWm,
        snapshot: &RuntimeStateSnapshot,
        window: &WindowSnapshot,
    ) -> Result<bool, ConfigError> {
        self.call_hook(
            state,
            snapshot,
            "window_unmapped",
            window_hook_context(&self.lua, "window_unmapped", snapshot, window)?,
        )
    }

    /// Trigger `evil.on.window_property_changed`.
    ///
    /// `state` is used to build the runtime snapshot. `ctx.window` reflects the **new** state.
    pub fn trigger_window_property_changed(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        property: &str,
        old_value: &PropertyValue,
        new_value: &PropertyValue,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "window_property_changed",
            property_changed_hook_context(
                &self.lua,
                &snapshot,
                &window,
                property,
                old_value,
                new_value,
            )?,
        )
    }

    pub fn trigger_resolve_focus(
        &self,
        state: &mut EvilWm,
        request: ResolveFocusRequest<'_>,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let previous_window = request
            .previous
            .and_then(|id| find_window_snapshot(&snapshot, id));
        self.call_hook(
            state,
            &snapshot,
            "resolve_focus",
            focus_resolve_context(
                &self.lua,
                ResolveFocusContext {
                    reason: request.reason,
                    state: &snapshot,
                    window: request.window,
                    previous: previous_window.as_ref(),
                    pointer: request.pointer,
                    button: request.button,
                    pressed: request.pressed,
                    modifiers: request.modifiers,
                },
            )?,
        )
    }

    pub fn trigger_focus_changed(
        &self,
        state: &mut EvilWm,
        previous: Option<WindowId>,
        current: Option<WindowId>,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let previous_window = previous.and_then(|id| find_window_snapshot(&snapshot, id));
        let current_window = current.and_then(|id| find_window_snapshot(&snapshot, id));
        self.call_hook(
            state,
            &snapshot,
            "focus_changed",
            focus_hook_context(
                &self.lua,
                &snapshot,
                previous_window.as_ref(),
                current_window.as_ref(),
            )?,
        )
    }

    pub fn trigger_key(&self, state: &mut EvilWm, keyspec: &str) -> Result<bool, ConfigError> {
        let (mods, key) = parse_keyspec(keyspec).map_err(ConfigError::Validation)?;
        let modifiers = ModifierSet::from_names(&mods);
        let snapshot = state.state_snapshot();
        let bound_action = state
            .bindings
            .resolve(&key, modifiers)
            .map(|action| action.name());
        self.call_hook(
            state,
            &snapshot,
            "key",
            key_hook_context(&self.lua, &snapshot, keyspec, &key, modifiers, bound_action)?,
        )
    }

    pub fn trigger_gesture(
        &self,
        state: &mut EvilWm,
        kind: &str,
        fingers: u32,
        delta: Vec2,
        scale: Option<f64>,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        self.call_hook(
            state,
            &snapshot,
            "gesture",
            gesture_hook_context(&self.lua, &snapshot, kind, fingers, delta, scale)?,
        )
    }

    pub fn trigger_move_begin(
        &self,
        state: &mut EvilWm,
        id: WindowId,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "move_begin",
            delta_hook_context(
                &self.lua,
                "move_begin",
                &snapshot,
                &window,
                Vec2::new(0.0, 0.0),
                None,
                None,
            )?,
        )
    }

    pub fn trigger_move_update(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "move_update",
            delta_hook_context(
                &self.lua,
                "move_update",
                &snapshot,
                &window,
                delta,
                pointer,
                None,
            )?,
        )
    }

    pub fn trigger_move_end(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "move_end",
            delta_hook_context(
                &self.lua, "move_end", &snapshot, &window, delta, pointer, None,
            )?,
        )
    }

    pub fn trigger_resize_begin(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "resize_begin",
            delta_hook_context(
                &self.lua,
                "resize_begin",
                &snapshot,
                &window,
                Vec2::new(0.0, 0.0),
                None,
                Some(edges),
            )?,
        )
    }

    pub fn trigger_resize_update(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "resize_update",
            delta_hook_context(
                &self.lua,
                "resize_update",
                &snapshot,
                &window,
                delta,
                pointer,
                Some(edges),
            )?,
        )
    }

    pub fn trigger_resize_end(
        &self,
        state: &mut EvilWm,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let snapshot = state.state_snapshot();
        let Some(window) = find_window_snapshot(&snapshot, id) else {
            return Ok(false);
        };
        self.call_hook(
            state,
            &snapshot,
            "resize_end",
            delta_hook_context(
                &self.lua,
                "resize_end",
                &snapshot,
                &window,
                delta,
                pointer,
                Some(edges),
            )?,
        )
    }

    fn call_hook(
        &self,
        state: &mut EvilWm,
        snapshot: &RuntimeStateSnapshot,
        hook_name: &str,
        context: Table,
    ) -> Result<bool, ConfigError> {
        let Some(callback) = self.lookup_hook(hook_name)? else {
            return Ok(false);
        };

        let previous_snapshot = self.query_snapshot.replace(Some(snapshot.clone()));
        self.action_queue.borrow_mut().clear();
        let result = match callback.call::<Value>(context).map_err(ConfigError::from) {
            Ok(value) => {
                let actions = self.drain_actions(value)?;
                for action in actions {
                    apply_action(state, action)?;
                }
                Ok(true)
            }
            Err(error) => Err(error),
        };
        self.query_snapshot.replace(previous_snapshot);
        result
    }

    fn call_draw_hook(
        &self,
        _state: &mut EvilWm,
        snapshot: &RuntimeStateSnapshot,
        hook_name: &str,
        context: Table,
    ) -> Result<Vec<DrawCommand>, ConfigError> {
        let Some(callback) = self.lookup_hook(hook_name)? else {
            return Ok(Vec::new());
        };

        let previous_snapshot = self.query_snapshot.replace(Some(snapshot.clone()));
        self.action_queue.borrow_mut().clear();
        let result = callback.call::<Value>(context).map_err(ConfigError::from);
        let queued_actions = self.action_queue.borrow_mut().drain(..).collect::<Vec<_>>();
        self.query_snapshot.replace(previous_snapshot);

        let value = result?;
        if !queued_actions.is_empty() {
            return Err(ConfigError::Validation(
                "draw hooks must be read-only and must not queue runtime actions".into(),
            ));
        }
        parse_draw_commands(value)
    }

    fn drain_actions(&self, result: Value) -> Result<Vec<HookAction>, ConfigError> {
        let mut actions = self.action_queue.borrow_mut().drain(..).collect::<Vec<_>>();
        actions.extend(parse_hook_actions(result)?);
        Ok(actions)
    }

    fn lookup_hook(&self, hook_name: &str) -> Result<Option<Function>, ConfigError> {
        let evil = self.lua.globals().get::<Table>("evil")?;
        let hooks = evil.get::<Table>("on")?;
        hooks
            .get::<Option<Function>>(hook_name)
            .map_err(ConfigError::from)
    }
}

fn apply_action(state: &mut EvilWm, action: HookAction) -> Result<(), ConfigError> {
    apply_hook_action(state, action)
}

fn register_include(lua: &Lua, base_dir: PathBuf) -> Result<(), ConfigError> {
    let include = lua.create_function(move |lua, relative_path: String| {
        let full_path =
            resolve_include_path(&base_dir, Path::new(&relative_path)).map_err(mlua::Error::external)?;
        let source = fs::read_to_string(&full_path).map_err(mlua::Error::external)?;
        lua.load(&source)
            .set_name(full_path.to_string_lossy().as_ref())
            .eval::<Value>()
    })?;
    lua.globals().set("include", include)?;
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

fn register_live_api(
    lua: &Lua,
    action_queue: &Rc<RefCell<Vec<HookAction>>>,
    query_snapshot: &Rc<RefCell<Option<RuntimeStateSnapshot>>>,
    allow_setup_calls_during_load: &Rc<RefCell<bool>>,
) -> Result<(), ConfigError> {
    let evil = lua.create_table()?;

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

    let state_snapshot = query_snapshot.clone();
    let state = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&state_snapshot)?;
        snapshot_to_table(lua, &snapshot)
    })?;
    evil.set("state", state)?;

    let pointer_table = lua.create_table()?;
    let pointer_snapshot = query_snapshot.clone();
    let position = lua.create_function(move |lua, ()| {
        let pointer = current_live_snapshot(&pointer_snapshot)?.pointer;
        let table = lua.create_table()?;
        table.set("x", pointer.x)?;
        table.set("y", pointer.y)?;
        Ok(table)
    })?;
    pointer_table.set("position", position)?;
    evil.set("pointer", pointer_table)?;

    let output_table = lua.create_table()?;
    let output_snapshot = query_snapshot.clone();
    let list_outputs = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&output_snapshot)?;
        let outputs = lua.create_table()?;
        for (index, output) in snapshot.outputs.iter().enumerate() {
            outputs.set(index + 1, output_to_table(lua, output)?)?;
        }
        Ok(outputs)
    })?;
    output_table.set("list", list_outputs)?;

    let get_output_snapshot = query_snapshot.clone();
    let get_output = lua.create_function(move |lua, id: String| {
        let snapshot = current_live_snapshot(&get_output_snapshot)?;
        find_output_snapshot(&snapshot, &id)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output_table.set("get", get_output)?;

    let primary_output_snapshot = query_snapshot.clone();
    let primary_output = lua.create_function(move |lua, ()| {
        let snapshot = current_live_snapshot(&primary_output_snapshot)?;
        find_primary_output_snapshot(&snapshot)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output_table.set("primary", primary_output)?;

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
    output_table.set("at_pointer", output_at_pointer)?;
    evil.set("output", output_table)?;

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
        let edges = crate::window::ResizeEdges {
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

    evil.set("on", lua.create_table()?)?;
    register_draw_api(lua, &evil)?;
    lua.globals().set("evil", evil)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::LiveLuaHooks;
    use crate::{
        canvas::Rect,
        lua::{
            DrawCommand, DrawSpace, HookAction, OutputSnapshot, PointerSnapshot,
            RuntimeStateSnapshot, ViewportSnapshot, WindowSnapshot, parse_draw_commands,
        },
    };
    use mlua::Value;

    #[test]
    fn imperative_live_api_calls_queue_actions() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        let value = hooks
            .eval_for_tests(
                r#"
                evil.window.move(7, 120, 240)
                evil.canvas.pan(5, -3)
                return true
                "#,
                "queue.lua",
            )
            .expect("eval live api");
        assert_eq!(value, Value::Boolean(true));

        let queued = hooks.action_queue.borrow().clone();
        assert_eq!(
            queued,
            vec![
                HookAction::MoveWindow {
                    id: 7,
                    x: 120.0,
                    y: 240.0,
                },
                HookAction::PanCanvas { dx: 5.0, dy: -3.0 },
            ]
        );
    }

    fn sample_snapshot() -> RuntimeStateSnapshot {
        RuntimeStateSnapshot {
            focused_window_id: Some(7),
            pointer: PointerSnapshot { x: 320.0, y: 180.0 },
            outputs: vec![OutputSnapshot {
                id: "nested".into(),
                logical_x: 0.0,
                logical_y: 0.0,
                viewport: ViewportSnapshot {
                    x: 64.0,
                    y: 96.0,
                    zoom: 1.25,
                    screen_w: 1280.0,
                    screen_h: 720.0,
                    visible_world: Rect::new(64.0, 96.0, 1024.0, 576.0),
                },
            }],
            windows: vec![WindowSnapshot {
                id: 7,
                app_id: Some("foot".into()),
                title: Some("shell".into()),
                bounds: Rect::new(100.0, 120.0, 900.0, 600.0),
                floating: true,
                exclude_from_focus: false,
                focused: true,
                fullscreen: false,
                maximized: false,
                urgent: false,
                mapped: true,
                mapped_at: Some(1_000_000.0),
                last_focused_at: None,
                output_id: Some("nested".into()),
                pid: None,
            }],
        }
    }

    #[test]
    fn live_query_helpers_read_current_hook_snapshot() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks.set_query_snapshot_for_tests(Some(sample_snapshot()));

        let value = hooks
            .eval_for_tests(
                r#"
                local state = evil.state()
                local pointer = evil.pointer.position()
                local outputs = evil.output.list()
                local windows = evil.window.list()
                local focused = evil.window.focused()
                local current = evil.window.get(7)
                local viewport = evil.canvas.viewport()
                return {
                  focused = state.focused_window_id,
                  pointer_x = pointer.x,
                  output_count = #outputs,
                  window_count = #windows,
                  focused_app_id = focused.app_id,
                  current_title = current.title,
                  viewport_zoom = viewport.zoom,
                }
                "#,
                "snapshot.lua",
            )
            .expect("eval live snapshot api");

        let Value::Table(table) = value else {
            panic!("expected table result");
        };

        assert_eq!(table.get::<u64>("focused").expect("focused id"), 7);
        assert_eq!(table.get::<f64>("pointer_x").expect("pointer x"), 320.0);
        assert_eq!(table.get::<i64>("output_count").expect("output count"), 1);
        assert_eq!(table.get::<i64>("window_count").expect("window count"), 1);
        assert_eq!(
            table
                .get::<String>("focused_app_id")
                .expect("focused app id"),
            "foot"
        );
        assert_eq!(
            table.get::<String>("current_title").expect("current title"),
            "shell"
        );
        assert_eq!(
            table.get::<f64>("viewport_zoom").expect("viewport zoom"),
            1.25
        );
    }

    #[test]
    fn live_output_helpers_cover_zero_single_and_multi_output_cases() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks.set_query_snapshot_for_tests(Some(RuntimeStateSnapshot {
            focused_window_id: None,
            pointer: PointerSnapshot { x: 0.0, y: 0.0 },
            outputs: Vec::new(),
            windows: Vec::new(),
        }));
        let zero_value = hooks
            .eval_for_tests(
                r#"
                return {
                  primary_nil = evil.output.primary() == nil,
                  pointer_nil = evil.output.at_pointer() == nil,
                  get_nil = evil.output.get("missing") == nil,
                }
                "#,
                "zero-outputs.lua",
            )
            .expect("eval zero outputs");
        let Value::Table(zero_table) = zero_value else {
            panic!("expected table result");
        };
        assert!(zero_table.get::<bool>("primary_nil").expect("primary_nil"));
        assert!(zero_table.get::<bool>("pointer_nil").expect("pointer_nil"));
        assert!(zero_table.get::<bool>("get_nil").expect("get_nil"));

        hooks.set_query_snapshot_for_tests(Some(sample_snapshot()));
        let single_value = hooks
            .eval_for_tests(
                r#"
                local primary = evil.output.primary()
                local pointer = evil.output.at_pointer()
                local got = evil.output.get("nested")
                return {
                  primary_id = primary.id,
                  pointer_id = pointer.id,
                  got_id = got.id,
                }
                "#,
                "single-output.lua",
            )
            .expect("eval single output");
        let Value::Table(single_table) = single_value else {
            panic!("expected table result");
        };
        assert_eq!(single_table.get::<String>("primary_id").expect("primary_id"), "nested");
        assert_eq!(single_table.get::<String>("pointer_id").expect("pointer_id"), "nested");
        assert_eq!(single_table.get::<String>("got_id").expect("got_id"), "nested");

        let mut multi = sample_snapshot();
        multi.outputs.push(OutputSnapshot {
            id: "side".into(),
            logical_x: 1280.0,
            logical_y: 0.0,
            viewport: ViewportSnapshot {
                x: 0.0,
                y: 0.0,
                zoom: 1.0,
                screen_w: 1920.0,
                screen_h: 1080.0,
                visible_world: Rect::new(0.0, 0.0, 1920.0, 1080.0),
            },
        });
        multi.pointer = PointerSnapshot { x: 1500.0, y: 200.0 };
        hooks.set_query_snapshot_for_tests(Some(multi));
        let multi_value = hooks
            .eval_for_tests(
                r#"
                local outputs = evil.output.list()
                local primary = evil.output.primary()
                local pointer = evil.output.at_pointer()
                local got = evil.output.get("side")
                return {
                  count = #outputs,
                  primary_id = primary.id,
                  pointer_id = pointer.id,
                  got_id = got.id,
                  bounds_x = got.bounds.x,
                  logical_bounds_x = got.logical_bounds.x,
                  screen_bounds_x = got.screen_bounds.x,
                  visible_world_w = got.visible_world.w,
                }
                "#,
                "multi-output.lua",
            )
            .expect("eval multi output");
        let Value::Table(multi_table) = multi_value else {
            panic!("expected table result");
        };
        assert_eq!(multi_table.get::<i64>("count").expect("count"), 2);
        assert_eq!(multi_table.get::<String>("primary_id").expect("primary_id"), "nested");
        assert_eq!(multi_table.get::<String>("pointer_id").expect("pointer_id"), "side");
        assert_eq!(multi_table.get::<String>("got_id").expect("got_id"), "side");
        assert_eq!(multi_table.get::<f64>("bounds_x").expect("bounds_x"), 1280.0);
        assert_eq!(
            multi_table
                .get::<f64>("logical_bounds_x")
                .expect("logical_bounds_x"),
            1280.0
        );
        assert_eq!(
            multi_table
                .get::<f64>("screen_bounds_x")
                .expect("screen_bounds_x"),
            0.0
        );
        assert_eq!(
            multi_table
                .get::<f64>("visible_world_w")
                .expect("visible_world_w"),
            1920.0
        );
    }

    #[test]
    fn draw_api_constructors_build_parseable_shapes() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        let value = hooks
            .eval_for_tests(
                r#"
                return {
                  evil.draw.rect({
                    space = "screen",
                    x = 10,
                    y = 20,
                    w = 30,
                    h = 40,
                    color = { 0.1, 0.2, 0.3, 0.4 },
                  }),
                  evil.draw.stroke_rect({
                    space = "world",
                    x = 50,
                    y = 60,
                    w = 70,
                    h = 80,
                    width = 3,
                    outer = 1,
                    color = { 0.7, 0.6, 0.5, 1.0 },
                  }),
                }
                "#,
                "draw-api.lua",
            )
            .expect("eval draw api");

        let commands = parse_draw_commands(value).expect("parse draw commands");
        assert_eq!(
            commands[0],
            DrawCommand::Rect {
                space: DrawSpace::Screen,
                x: 10.0,
                y: 20.0,
                w: 30.0,
                h: 40.0,
                color: [0.1, 0.2, 0.3, 0.4],
            }
        );
        assert_eq!(
            commands[1],
            DrawCommand::StrokeRect {
                space: DrawSpace::World,
                x: 50.0,
                y: 60.0,
                w: 70.0,
                h: 80.0,
                width: 3.0,
                outer: 1.0,
                color: [0.7, 0.6, 0.5, 1.0],
            }
        );
    }

    #[test]
    fn config_time_setup_api_errors_in_live_runtime() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");

        for (source, expected) in [
            ("evil.config({})", "evil.config() is config-time only"),
            ("evil.bind('Super+H', 'pan_left')", "evil.bind() is config-time only"),
            ("evil.autostart('foot')", "evil.autostart() is config-time only"),
        ] {
            let error = hooks
                .eval_for_tests(source, "config-time-only.lua")
                .expect_err("setup API should fail in live runtime");
            assert!(
                error.to_string().contains(expected),
                "expected error containing {expected:?}, got: {error}"
            );
        }
    }

    #[test]
    fn live_query_helpers_error_outside_hook_context() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        let error = hooks
            .eval_for_tests("return evil.state()", "outside.lua")
            .expect_err("state query should fail without active hook snapshot");
        let message = error.to_string();
        assert!(message.contains("live query helpers are only available while a hook is running"));
    }

    #[test]
    fn live_hook_registration_detection_matches_script_contents() {
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.window_mapped = function(ctx)
                  return nil
                end
                "#,
                "hooks.lua",
            )
            .expect("load hook script");
        assert!(
            hooks
                .has_hook("window_mapped")
                .expect("window_mapped lookup")
        );
        assert!(!hooks.has_hook("resize_end").expect("resize_end lookup"));
    }
}
