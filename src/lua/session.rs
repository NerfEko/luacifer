use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use mlua::{Function, Lua, Table, Value};

use super::live::ResolveFocusRequest;

use crate::{
    canvas::{Point, Rect, Vec2},
    headless::HeadlessSession,
    input::{ModifierSet, parse_keyspec},
    lua::{
        ConfigError, HookAction, PropertyValue, RuntimeStateSnapshot, WindowSnapshot,
        api::ActionTarget, apply_hook_action, register_draw_api,
        hook_support::{
            ResolveFocusContext, delta_hook_context, find_output_snapshot,
            find_output_snapshot_at_point, find_primary_output_snapshot, find_window_snapshot,
            focus_hook_context, focus_resolve_context, gesture_hook_context, key_hook_context,
            output_to_table, property_changed_hook_context, rect_to_table, snapshot_to_table,
            window_hook_context, window_to_table,
        },
        parse_hook_actions,
    },
    window::{ResizeEdges, WindowId},
};

#[derive(Debug)]
pub struct LuaSession {
    lua: Lua,
    _base_dir: PathBuf,
    session: Rc<RefCell<HeadlessSession>>,
}

impl LuaSession {
    pub fn new(
        base_dir: impl Into<PathBuf>,
        session: Rc<RefCell<HeadlessSession>>,
    ) -> Result<Self, ConfigError> {
        let lua = Lua::new();
        let base_dir = base_dir.into();

        register_runtime_api(&lua, session.clone())?;

        Ok(Self {
            lua,
            _base_dir: base_dir,
            session,
        })
    }

    pub fn eval(&self, source: &str, name: impl AsRef<Path>) -> Result<Value, ConfigError> {
        let name = name.as_ref();
        self.lua
            .load(source)
            .set_name(name.to_string_lossy().as_ref())
            .eval::<Value>()
            .map_err(ConfigError::from)
    }

    pub fn session(&self) -> Rc<RefCell<HeadlessSession>> {
        self.session.clone()
    }

    pub fn trigger_place_window(&self, id: WindowId) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "place_window",
            window_hook_context(&self.lua, "place_window", &state, &window)?,
        )
    }

    pub fn trigger_window_mapped(&self, id: WindowId) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "window_mapped",
            window_hook_context(&self.lua, "window_mapped", &state, &window)?,
        )
    }

    pub fn trigger_window_unmapped(&self, id: WindowId) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.trigger_window_unmapped_snapshot(&state, &window)
    }

    pub fn trigger_window_unmapped_snapshot(
        &self,
        state: &RuntimeStateSnapshot,
        window: &WindowSnapshot,
    ) -> Result<bool, ConfigError> {
        self.call_hook(
            "window_unmapped",
            window_hook_context(&self.lua, "window_unmapped", state, window)?,
        )
    }

    /// Trigger `evil.on.window_property_changed` in the headless runtime.
    ///
    /// `ctx.window` reflects the **current** (new) state of the window.
    pub fn trigger_window_property_changed(
        &self,
        id: WindowId,
        property: &str,
        old_value: &PropertyValue,
        new_value: &PropertyValue,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "window_property_changed",
            property_changed_hook_context(
                &self.lua,
                &state,
                &window,
                property,
                old_value,
                new_value,
            )?,
        )
    }

    pub fn trigger_resolve_focus(&self, request: ResolveFocusRequest<'_>) -> Result<bool, ConfigError> {
        let state = self.session.borrow().state_snapshot();
        let previous_window = request.previous.and_then(|id| find_window_snapshot(&state, id));
        self.call_hook(
            "resolve_focus",
            focus_resolve_context(
                &self.lua,
                ResolveFocusContext {
                    reason: request.reason,
                    state: &state,
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
        previous: Option<WindowId>,
        current: Option<WindowId>,
    ) -> Result<bool, ConfigError> {
        let state = self.session.borrow().state_snapshot();
        let previous_window = previous.and_then(|id| find_window_snapshot(&state, id));
        let current_window = current.and_then(|id| find_window_snapshot(&state, id));
        self.call_hook(
            "focus_changed",
            focus_hook_context(
                &self.lua,
                &state,
                previous_window.as_ref(),
                current_window.as_ref(),
            )?,
        )
    }

    pub fn trigger_move_begin(&self, id: WindowId) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "move_begin",
            delta_hook_context(
                &self.lua,
                "move_begin",
                &state,
                &window,
                Vec2::new(0.0, 0.0),
                None,
                None,
            )?,
        )
    }

    pub fn trigger_move_update(
        &self,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "move_update",
            delta_hook_context(
                &self.lua,
                "move_update",
                &state,
                &window,
                delta,
                pointer,
                None,
            )?,
        )
    }

    pub fn trigger_move_end(
        &self,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "move_end",
            delta_hook_context(&self.lua, "move_end", &state, &window, delta, pointer, None)?,
        )
    }

    pub fn trigger_resize_begin(
        &self,
        id: WindowId,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "resize_begin",
            delta_hook_context(
                &self.lua,
                "resize_begin",
                &state,
                &window,
                Vec2::new(0.0, 0.0),
                None,
                Some(edges),
            )?,
        )
    }

    pub fn trigger_resize_update(
        &self,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "resize_update",
            delta_hook_context(
                &self.lua,
                "resize_update",
                &state,
                &window,
                delta,
                pointer,
                Some(edges),
            )?,
        )
    }

    pub fn trigger_resize_end(
        &self,
        id: WindowId,
        delta: Vec2,
        pointer: Option<Point>,
        edges: ResizeEdges,
    ) -> Result<bool, ConfigError> {
        let (state, window) = self.snapshot_with_window(id)?;
        self.call_hook(
            "resize_end",
            delta_hook_context(
                &self.lua,
                "resize_end",
                &state,
                &window,
                delta,
                pointer,
                Some(edges),
            )?,
        )
    }

    pub fn trigger_key(&self, keyspec: &str) -> Result<bool, ConfigError> {
        let (mods, key) = parse_keyspec(keyspec).map_err(ConfigError::Validation)?;
        let modifiers = ModifierSet::from_names(&mods);
        let state = self.session.borrow().state_snapshot();
        let bound_action = self
            .session
            .borrow()
            .bindings
            .resolve(&key, modifiers)
            .map(|action| action.name());
        self.call_hook(
            "key",
            key_hook_context(&self.lua, &state, keyspec, &key, modifiers, bound_action)?,
        )
    }

    pub fn trigger_gesture(
        &self,
        kind: &str,
        fingers: u32,
        delta: Vec2,
        scale: Option<f64>,
    ) -> Result<bool, ConfigError> {
        let state = self.session.borrow().state_snapshot();
        self.call_hook(
            "gesture",
            gesture_hook_context(&self.lua, &state, kind, fingers, delta, scale)?,
        )
    }

    fn snapshot_with_window(
        &self,
        id: WindowId,
    ) -> Result<(RuntimeStateSnapshot, WindowSnapshot), ConfigError> {
        let state = self.session.borrow().state_snapshot();
        let window = state
            .windows
            .iter()
            .find(|window| window.id == id.0)
            .cloned()
            .ok_or_else(|| ConfigError::Validation(format!("unknown window id: {}", id.0)))?;
        Ok((state, window))
    }

    fn call_hook(&self, hook_name: &str, context: Table) -> Result<bool, ConfigError> {
        let Some(callback) = self.lookup_hook(hook_name)? else {
            return Ok(false);
        };

        let result = callback.call::<Value>(context).map_err(ConfigError::from)?;
        self.apply_hook_result(result)?;
        Ok(true)
    }

    fn apply_hook_result(&self, result: Value) -> Result<(), ConfigError> {
        for action in parse_hook_actions(result)? {
            self.apply_action(action)?;
        }
        Ok(())
    }

    fn apply_action(&self, action: HookAction) -> Result<(), ConfigError> {
        let mut session = self.session.borrow_mut();
        apply_hook_action(&mut *session, action)
    }

    fn lookup_hook(&self, hook_name: &str) -> Result<Option<Function>, ConfigError> {
        let evil = self.lua.globals().get::<Table>("evil")?;
        let hooks = evil.get::<Table>("on")?;
        hooks
            .get::<Option<Function>>(hook_name)
            .map_err(ConfigError::from)
    }
}

fn register_runtime_api(
    lua: &Lua,
    session: Rc<RefCell<HeadlessSession>>,
) -> Result<(), ConfigError> {
    let evil = lua.create_table()?;

    let state_session = session.clone();
    let state = lua.create_function(move |lua, ()| {
        let snapshot = state_session.borrow().state_snapshot();
        snapshot_to_table(lua, &snapshot)
    })?;
    evil.set("state", state)?;

    let pointer_table = lua.create_table()?;
    let pointer_session = session.clone();
    let position = lua.create_function(move |lua, ()| {
        let pointer = pointer_session.borrow().state_snapshot().pointer;
        let table = lua.create_table()?;
        table.set("x", pointer.x)?;
        table.set("y", pointer.y)?;
        Ok(table)
    })?;
    pointer_table.set("position", position)?;
    evil.set("pointer", pointer_table)?;

    let output_table = lua.create_table()?;
    let output_session = session.clone();
    let list_outputs = lua.create_function(move |lua, ()| {
        let snapshot = output_session.borrow().state_snapshot();
        let outputs = lua.create_table()?;
        for (index, output) in snapshot.outputs.iter().enumerate() {
            outputs.set(index + 1, output_to_table(lua, output)?)?;
        }
        Ok(outputs)
    })?;
    output_table.set("list", list_outputs)?;

    let get_output_session = session.clone();
    let get_output = lua.create_function(move |lua, id: String| {
        let snapshot = get_output_session.borrow().state_snapshot();
        find_output_snapshot(&snapshot, &id)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output_table.set("get", get_output)?;

    let primary_output_session = session.clone();
    let primary_output = lua.create_function(move |lua, ()| {
        let snapshot = primary_output_session.borrow().state_snapshot();
        find_primary_output_snapshot(&snapshot)
            .map(|output| output_to_table(lua, &output))
            .transpose()
    })?;
    output_table.set("primary", primary_output)?;

    let pointer_output_session = session.clone();
    let output_at_pointer = lua.create_function(move |lua, ()| {
        let snapshot = pointer_output_session.borrow().state_snapshot();
        find_output_snapshot_at_point(
            &snapshot,
            Point::new(snapshot.pointer.x, snapshot.pointer.y),
        )
        .map(|output| output_to_table(lua, &output))
        .transpose()
    })?;
    output_table.set("at_pointer", output_at_pointer)?;
    evil.set("output", output_table)?;

    let window_table = lua.create_table()?;

    let list_session = session.clone();
    let list = lua.create_function(move |lua, ()| {
        let snapshot = list_session.borrow().state_snapshot();
        let windows = lua.create_table()?;
        for (index, window) in snapshot.windows.iter().enumerate() {
            windows.set(index + 1, window_to_table(lua, window)?)?;
        }
        Ok(windows)
    })?;
    window_table.set("list", list)?;

    let get_session = session.clone();
    let get = lua.create_function(move |lua, id: u64| {
        let snapshot = get_session.borrow().state_snapshot();
        snapshot
            .windows
            .iter()
            .find(|window| window.id == id)
            .map(|window| window_to_table(lua, window))
            .transpose()
    })?;
    window_table.set("get", get)?;

    let focused_session = session.clone();
    let focused = lua.create_function(move |lua, ()| {
        let snapshot = focused_session.borrow().state_snapshot();
        snapshot
            .focused_window_id
            .and_then(|id| snapshot.windows.iter().find(|window| window.id == id))
            .map(|window| window_to_table(lua, window))
            .transpose()
    })?;
    window_table.set("focused", focused)?;

    let focus_session = session.clone();
    let focus = lua.create_function(move |_, id: u64| {
        Ok(focus_session.borrow_mut().focus_window(WindowId(id)))
    })?;
    window_table.set("focus", focus)?;

    let clear_focus_session = session.clone();
    let clear_focus = lua.create_function(move |_, ()| {
        Ok(clear_focus_session.borrow_mut().clear_focus())
    })?;
    window_table.set("clear_focus", clear_focus)?;

    let move_session = session.clone();
    let move_window = lua.create_function(move |_, (id, x, y): (u64, f64, f64)| {
        Ok(move_session.borrow_mut().move_window(WindowId(id), x, y))
    })?;
    window_table.set("move", move_window)?;

    let resize_session = session.clone();
    let resize = lua.create_function(move |_, (id, w, h): (u64, f64, f64)| {
        Ok(resize_session
            .borrow_mut()
            .resize_window(WindowId(id), w, h))
    })?;
    window_table.set("resize", resize)?;

    let bounds_session = session.clone();
    let set_bounds =
        lua.create_function(move |_, (id, x, y, w, h): (u64, f64, f64, f64, f64)| {
            Ok(bounds_session
                .borrow_mut()
                .set_window_bounds(WindowId(id), Rect::new(x, y, w, h)))
        })?;
    window_table.set("set_bounds", set_bounds)?;

    let begin_move_session = session.clone();
    let begin_move = lua.create_function(move |_, id: u64| {
        Ok(begin_move_session
            .borrow_mut()
            .begin_interactive_move(WindowId(id)))
    })?;
    window_table.set("begin_move", begin_move)?;

    let begin_resize_session = session.clone();
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
        Ok(begin_resize_session
            .borrow_mut()
            .begin_interactive_resize(WindowId(id), edges))
    })?;
    window_table.set("begin_resize", begin_resize)?;

    let close_session = session.clone();
    let close = lua.create_function(move |_, id: u64| {
        Ok(close_session.borrow_mut().close_window(WindowId(id)))
    })?;
    window_table.set("close", close)?;

    evil.set("window", window_table)?;

    let canvas_table = lua.create_table()?;

    let viewport_session = session.clone();
    let viewport = lua.create_function(move |lua, ()| {
        let snapshot = viewport_session.borrow().state_snapshot();
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
    canvas_table.set("viewport", viewport)?;

    let pan_session = session.clone();
    let pan = lua.create_function(move |_, (dx, dy): (f64, f64)| {
        pan_session
            .borrow_mut()
            .viewport_mut()
            .pan_world(Vec2::new(dx, dy));
        Ok(true)
    })?;
    canvas_table.set("pan", pan)?;

    let zoom_session = session.clone();
    let zoom = lua.create_function(move |_, factor: f64| {
        if factor <= 0.0 {
            return Ok(false);
        }
        let mut session = zoom_session.borrow_mut();
        let screen = session.viewport().screen_size();
        session
            .viewport_mut()
            .zoom_at_screen(Point::new(screen.w / 2.0, screen.h / 2.0), factor);
        Ok(true)
    })?;
    canvas_table.set("zoom", zoom)?;

    evil.set("canvas", canvas_table)?;
    evil.set("on", lua.create_table()?)?;
    register_draw_api(lua, &evil)?;

    lua.globals().set("evil", evil)?;
    Ok(())
}
