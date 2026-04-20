use super::*;

impl EvilWm {
    #[cfg(feature = "lua")]
    pub(crate) fn with_live_lua<R, F>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> R,
    {
        let hooks = self.live_lua.take()?;
        let result = f(&hooks, self);
        self.live_lua = Some(hooks);
        Some(result)
    }

    #[cfg(feature = "lua")]
    pub(crate) fn run_live_hook<F>(&mut self, hook_name: &str, f: F)
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> Result<bool, crate::lua::ConfigError>,
    {
        if let Some(result) = self.with_live_lua(f)
            && let Err(error) = result
        {
            self.record_live_hook_error(hook_name, &error);
        }
    }

    #[cfg(feature = "lua")]
    pub(crate) fn run_live_hook_result<F>(&mut self, hook_name: &str, f: F) -> bool
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> Result<bool, crate::lua::ConfigError>,
    {
        self.with_live_lua(f)
            .map(|result| match result {
                Ok(handled) => handled,
                Err(error) => {
                    self.record_live_hook_error(hook_name, &error);
                    false
                }
            })
            .unwrap_or(false)
    }

    pub(crate) fn trigger_live_place_window(&mut self, id: WindowId) {
        #[cfg(feature = "lua")]
        self.run_live_hook("place_window", |hooks, state| {
            hooks.trigger_place_window(state, id)
        });
    }

    pub(crate) fn trigger_live_window_mapped(&mut self, id: WindowId) {
        self.emit_event(
            "window_mapped",
            serde_json::json!({
                "id": id.0,
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("window_mapped", |hooks, state| {
            hooks.trigger_window_mapped(state, id)
        });
    }

    pub(crate) fn trigger_live_window_unmapped(
        &mut self,
        snapshot: RuntimeStateSnapshot,
        window: WindowSnapshot,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("window_unmapped", |hooks, state| {
            hooks.trigger_window_unmapped(state, &snapshot, &window)
        });
    }

    pub(crate) fn trigger_live_window_property_changed(
        &mut self,
        id: WindowId,
        property: &'static str,
        old_value: crate::lua::PropertyValue,
        new_value: crate::lua::PropertyValue,
    ) {
        self.emit_event(
            "window_property_changed",
            serde_json::json!({
                "id": id.0,
                "property": property,
                "old_value": old_value.to_json_value(),
                "new_value": new_value.to_json_value(),
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("window_property_changed", |hooks, state| {
            hooks.trigger_window_property_changed(state, id, property, &old_value, &new_value)
        });
    }

    pub(crate) fn trigger_live_focus_changed(
        &mut self,
        previous: Option<WindowId>,
        current: Option<WindowId>,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("focus_changed", |hooks, state| {
            hooks.trigger_focus_changed(state, previous, current)
        });
    }

    pub(crate) fn trigger_live_key(&mut self, keyspec: String) -> bool {
        #[cfg(feature = "lua")]
        {
            self.run_live_hook_result("key", |hooks, state| hooks.trigger_key(state, &keyspec))
        }

        #[cfg(not(feature = "lua"))]
        {
            let _ = keyspec;
            false
        }
    }

    pub(crate) fn trigger_live_gesture(
        &mut self,
        kind: &str,
        fingers: u32,
        delta: crate::canvas::Vec2,
        scale: Option<f64>,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("gesture", |hooks, state| {
            hooks.trigger_gesture(state, kind, fingers, delta, scale)
        });
    }

    pub(crate) fn advance_active_interactive_op(&mut self, current: Point) {
        let pending_trigger = if let Some(active) = self.active_interactive_op.as_mut() {
            let delta = active.advance(current);
            if delta.x != 0.0 || delta.y != 0.0 {
                Some((active.kind, active.window_id, delta, active.resize_edges()))
            } else {
                None
            }
        } else {
            None
        };

        if let Some((kind, id, delta, resize_edges)) = pending_trigger {
            match kind {
                ActiveInteractiveKind::Move => self.trigger_live_move_update(id, delta, current),
                ActiveInteractiveKind::Resize => self.trigger_live_resize_update(
                    id,
                    delta,
                    current,
                    resize_edges.unwrap_or(ResizeEdges::all()),
                ),
                ActiveInteractiveKind::PanCanvas => {
                    let pan_delta = if self.is_tty_backend() {
                        crate::canvas::Vec2::new(-delta.x, -delta.y)
                    } else {
                        let zoom = self.viewport().zoom().max(f64::EPSILON);
                        crate::canvas::Vec2::new(delta.x / zoom, delta.y / zoom)
                    };
                    if let Some(primary_name) = self.primary_output_name()
                        && let Some(state) = self.output_state_for_name_mut(&primary_name)
                    {
                        state.viewport_mut().pan_world(pan_delta);
                        self.clone_primary_camera_to_all_outputs();
                    } else {
                        self.output_state.viewport_mut().pan_world(pan_delta);
                    }
                    if self.is_tty_backend() {
                        let previous_pointer = Point::new(current.x - delta.x, current.y - delta.y);
                        if let Some(pointer) = self.seat.get_pointer() {
                            pointer.set_location((previous_pointer.x, previous_pointer.y).into());
                        }
                        if let Some(active) = self.active_interactive_op.as_mut() {
                            active.last_pointer = previous_pointer;
                        }
                    }
                    self.request_redraw();
                }
            }
            if kind != ActiveInteractiveKind::PanCanvas && !self.window_models.contains_key(&id) {
                self.active_interactive_op = None;
            }
        }
    }

    pub(crate) fn finish_active_interactive_op(&mut self, button: u32, pointer_pos: Point) {
        if self
            .active_interactive_op
            .as_ref()
            .is_some_and(|active| active.should_end_on_button(button))
        {
            let Some(active) = self.active_interactive_op.take() else {
                return;
            };
            match active.kind {
                ActiveInteractiveKind::Move => {
                    self.trigger_live_move_end(active.window_id, active.total_delta(), pointer_pos)
                }
                ActiveInteractiveKind::Resize => self.trigger_live_resize_end(
                    active.window_id,
                    active.total_delta(),
                    pointer_pos,
                    active.resize_edges().unwrap_or(ResizeEdges::all()),
                ),
                ActiveInteractiveKind::PanCanvas => {}
            }
        }
    }

    pub(crate) fn trigger_live_move_begin(&mut self, id: WindowId) {
        self.emit_event(
            "interactive_move_begin",
            serde_json::json!({
                "id": id.0,
            }),
        );
        #[cfg(feature = "lua")]
        if !self.live_hook_exists("move_update") {
            self.warn_missing_move_update_hook();
        }

        #[cfg(feature = "lua")]
        self.run_live_hook("move_begin", |hooks, state| {
            hooks.trigger_move_begin(state, id)
        });
    }

    pub(crate) fn trigger_live_move_update(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
    ) {
        self.emit_event(
            "interactive_move_update",
            serde_json::json!({
                "id": id.0,
                "delta": { "x": delta.x, "y": delta.y },
                "pointer": { "x": pointer.x, "y": pointer.y },
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("move_update", |hooks, state| {
            hooks.trigger_move_update(state, id, delta, Some(pointer))
        });
    }

    pub(crate) fn trigger_live_move_end(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
    ) {
        self.emit_event(
            "interactive_move_end",
            serde_json::json!({
                "id": id.0,
                "delta": { "x": delta.x, "y": delta.y },
                "pointer": { "x": pointer.x, "y": pointer.y },
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("move_end", |hooks, state| {
            hooks.trigger_move_end(state, id, delta, Some(pointer))
        });
    }

    pub(crate) fn trigger_live_resize_begin(&mut self, id: WindowId, edges: ResizeEdges) {
        self.emit_event(
            "interactive_resize_begin",
            serde_json::json!({
                "id": id.0,
                "edges": {
                    "left": edges.left,
                    "right": edges.right,
                    "top": edges.top,
                    "bottom": edges.bottom,
                },
            }),
        );
        #[cfg(feature = "lua")]
        if !self.live_hook_exists("resize_update") {
            self.warn_missing_resize_update_hook();
        }

        #[cfg(feature = "lua")]
        self.run_live_hook("resize_begin", |hooks, state| {
            hooks.trigger_resize_begin(state, id, edges)
        });
    }

    pub(crate) fn trigger_live_resize_update(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
        edges: ResizeEdges,
    ) {
        self.emit_event(
            "interactive_resize_update",
            serde_json::json!({
                "id": id.0,
                "delta": { "x": delta.x, "y": delta.y },
                "pointer": { "x": pointer.x, "y": pointer.y },
                "edges": {
                    "left": edges.left,
                    "right": edges.right,
                    "top": edges.top,
                    "bottom": edges.bottom,
                },
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("resize_update", |hooks, state| {
            hooks.trigger_resize_update(state, id, delta, Some(pointer), edges)
        });
    }

    pub(crate) fn trigger_live_resize_end(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
        edges: ResizeEdges,
    ) {
        self.emit_event(
            "interactive_resize_end",
            serde_json::json!({
                "id": id.0,
                "delta": { "x": delta.x, "y": delta.y },
                "pointer": { "x": pointer.x, "y": pointer.y },
                "edges": {
                    "left": edges.left,
                    "right": edges.right,
                    "top": edges.top,
                    "bottom": edges.bottom,
                },
            }),
        );
        #[cfg(feature = "lua")]
        self.run_live_hook("resize_end", |hooks, state| {
            hooks.trigger_resize_end(state, id, delta, Some(pointer), edges)
        });
    }
}

#[cfg(feature = "lua")]
pub(super) fn format_live_hook_error(hook_name: &str, error: &crate::lua::ConfigError) -> String {
    format!("[evilwm] lua hook error: evil.on.{hook_name} — {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::PropertyValue;
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};

    #[test]
    fn property_change_event_log_uses_structured_json_values() {
        let log_dir = tempfile::tempdir().expect("tempdir");
        let log_path = log_dir.path().join("events.jsonl");
        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        let mut state = EvilWm::new(&mut event_loop, display, None, None).expect("state");
        state.event_log_path = Some(log_path.clone());

        state.trigger_live_window_property_changed(
            WindowId(7),
            "title",
            PropertyValue::OptionString(None),
            PropertyValue::OptionString(Some("shell".into())),
        );
        state.trigger_live_window_property_changed(
            WindowId(7),
            "floating",
            PropertyValue::Bool(false),
            PropertyValue::Bool(true),
        );

        let entries = std::fs::read_to_string(&log_path)
            .expect("read event log")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .collect::<Vec<_>>();
        let title = entries
            .iter()
            .find(|entry| {
                entry["kind"] == "window_property_changed" && entry["data"]["property"] == "title"
            })
            .expect("title property event");
        assert!(title["data"]["old_value"].is_null());
        assert_eq!(title["data"]["new_value"], serde_json::json!("shell"));

        let floating = entries
            .iter()
            .find(|entry| {
                entry["kind"] == "window_property_changed"
                    && entry["data"]["property"] == "floating"
            })
            .expect("floating property event");
        assert_eq!(floating["data"]["old_value"], serde_json::json!(false));
        assert_eq!(floating["data"]["new_value"], serde_json::json!(true));
    }
}
