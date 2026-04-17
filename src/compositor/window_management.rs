use super::*;

impl EvilWm {
    pub fn focus_window(&mut self, id: WindowId) -> bool {
        if !self.window_models.contains_key(&id) {
            return false;
        }

        let previous = self.focus_stack.focused();
        self.focus_stack.focus(id);
        if let Some(window) = self.window_models.get_mut(&id) {
            window.last_focused_at = Some(std::time::SystemTime::now());
        }
        self.sync_focus_to_stack();
        self.emit_event(
            "focus_changed",
            serde_json::json!({
                "previous": previous.map(|window| window.0),
                "current": Some(id.0),
            }),
        );
        self.trigger_live_focus_changed(previous, Some(id));
        self.request_redraw();
        true
    }

    pub fn clear_focus(&mut self) -> bool {
        let previous = self.focus_stack.focused();
        if previous.is_none() {
            return false;
        }

        self.focus_stack.clear_focus_only();
        self.sync_focus_to_stack();
        self.emit_event(
            "focus_changed",
            serde_json::json!({
                "previous": previous.map(|window| window.0),
                "current": serde_json::Value::Null,
            }),
        );
        self.trigger_live_focus_changed(previous, None);
        self.request_redraw();
        true
    }

    pub fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        if let Some(window) = self.window_models.get_mut(&id) {
            window.bounds.origin = Point::new(x, y);
            if let Some(space_window) = self.find_space_window(id) {
                let loc = (x.round() as i32, y.round() as i32);
                self.space.map_element(space_window.clone(), loc, true);
                #[cfg(feature = "xwayland")]
                if let Some(surface) = space_window.x11_surface() {
                    let mut geometry = surface.geometry();
                    geometry.loc = loc.into();
                    let _ = surface.configure(geometry);
                }
            }
            self.request_redraw();
            true
        } else {
            false
        }
    }

    pub fn resize_window(&mut self, id: WindowId, w: f64, h: f64) -> bool {
        if w <= 0.0 || h <= 0.0 {
            return false;
        }
        if let Some(window) = self.window_models.get_mut(&id) {
            window.bounds.size = Size::new(w, h);
            if let Some(space_window) = self.find_space_window(id) {
                if let Some(toplevel) = window_toplevel(&space_window) {
                    toplevel.with_pending_state(|state| {
                        state.size = Some((w.round() as i32, h.round() as i32).into());
                    });
                    toplevel.send_configure();
                }
                #[cfg(feature = "xwayland")]
                if let Some(surface) = space_window.x11_surface() {
                    let mut geometry = surface.geometry();
                    geometry.size = (w.round() as i32, h.round() as i32).into();
                    let _ = surface.configure(geometry);
                }
            }
            self.remember_window_size_for_id(id);
            self.request_redraw();
            true
        } else {
            false
        }
    }

    pub fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool {
        if bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
            return false;
        }
        self.move_window(id, bounds.origin.x, bounds.origin.y)
            && self.resize_window(id, bounds.size.w, bounds.size.h)
    }

    pub fn close_window(&mut self, id: WindowId) -> bool {
        if !self.window_surfaces.contains_key(&id) {
            return false;
        }

        if let Some(space_window) = self.find_space_window(id) {
            if let Some(toplevel) = window_toplevel(&space_window) {
                toplevel.send_close();
                return true;
            }
            #[cfg(feature = "xwayland")]
            if let Some(surface) = space_window.x11_surface() {
                let _ = surface.close();
                return true;
            }
        }

        false
    }

    pub(crate) fn sync_focus_to_stack(&mut self) {
        let target_id = self.focus_stack.focused();
        let target_surface = target_id.and_then(|id| self.window_surfaces.get(&id).cloned());

        if let Some(id) = target_id
            && let Some(window) = self.find_space_window(id)
        {
            self.space.raise_element(&window, true);
        }

        self.space.elements().for_each(|window| {
            let activated = target_surface
                .as_ref()
                .is_some_and(|surface| window_matches_surface(window, surface));
            window.set_activated(activated);
            if let Some(toplevel) = window_toplevel(window) {
                toplevel.send_pending_configure();
            }
        });

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, target_surface, SERIAL_COUNTER.next_serial());
        }
    }

    pub(crate) fn handle_action(&mut self, action: crate::input::Action) {
        match action {
            crate::input::Action::CloseWindow => self.close_focused_window(),
            crate::input::Action::Spawn { command } => {
                spawn_client(&command, &self.socket_name, &self.ipc_socket_path)
            }
            crate::input::Action::PanLeft { amount } => {
                self.pan_all_viewports(crate::canvas::Vec2::new(-amount, 0.0))
            }
            crate::input::Action::PanRight { amount } => {
                self.pan_all_viewports(crate::canvas::Vec2::new(amount, 0.0))
            }
            crate::input::Action::PanUp { amount } => {
                self.pan_all_viewports(crate::canvas::Vec2::new(0.0, -amount))
            }
            crate::input::Action::PanDown { amount } => {
                self.pan_all_viewports(crate::canvas::Vec2::new(0.0, amount))
            }
            crate::input::Action::ZoomIn { factor } | crate::input::Action::ZoomOut { factor } => {
                let anchor = Point::new(
                    self.viewport().screen_size().w / 2.0,
                    self.viewport().screen_size().h / 2.0,
                );
                self.zoom_all_viewports_at_primary(anchor, factor);
            }
        }
        self.request_redraw();
    }

    pub(crate) fn close_focused_window(&mut self) {
        if let Some(id) = self.focus_stack.focused() {
            let _ = self.close_window(id);
        }
    }

    pub fn begin_interactive_move(&mut self, id: WindowId) -> bool {
        if !self.window_models.contains_key(&id) {
            return false;
        }
        let pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_else(|| (0.0, 0.0).into());
        self.active_interactive_op = Some(ActiveInteractiveOp::new(
            ActiveInteractiveKind::Move,
            id,
            Point::new(pointer.x, pointer.y),
            None,
            self.last_pointer_button_pressed,
        ));
        self.trigger_live_move_begin(id);
        true
    }

    pub fn begin_interactive_resize(&mut self, id: WindowId, edges: ResizeEdges) -> bool {
        if !self.window_models.contains_key(&id) {
            return false;
        }
        let pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_else(|| (0.0, 0.0).into());
        self.active_interactive_op = Some(ActiveInteractiveOp::new(
            ActiveInteractiveKind::Resize,
            id,
            Point::new(pointer.x, pointer.y),
            Some(edges),
            self.last_pointer_button_pressed,
        ));
        self.trigger_live_resize_begin(id, edges);
        true
    }

    pub(crate) fn sizing_config(&self) -> crate::lua::WindowConfig {
        self.config
            .as_ref()
            .map(|config| config.window.clone())
            .unwrap_or_default()
    }

    pub(crate) fn size_memory_key(properties: &WindowProperties) -> Option<String> {
        properties
            .app_id
            .as_deref()
            .map(str::trim)
            .filter(|app_id| !app_id.is_empty())
            .map(ToOwned::to_owned)
    }

    pub(crate) fn client_preferred_size(surface: &ToplevelSurface) -> Option<Size> {
        let geometry = Window::new_wayland_window(surface.clone()).geometry();
        (geometry.size.w > 0 && geometry.size.h > 0)
            .then(|| Size::new(geometry.size.w as f64, geometry.size.h as f64))
    }

    pub(crate) fn requested_initial_window_size_from(
        &self,
        properties: &WindowProperties,
        client_preferred_size: Option<Size>,
    ) -> Option<Size> {
        let applied_rules = AppliedWindowRules::from_rules(properties, &self.window_rules);
        if let Some(size) = applied_rules.default_size {
            return Some(size);
        }

        let sizing = self.sizing_config();
        if sizing.remember_sizes_by_app_id
            && let Some(key) = Self::size_memory_key(properties)
            && let Some(size) = self.remembered_app_sizes.get(&key)
        {
            return Some(*size);
        }

        if sizing.use_client_default_size {
            return client_preferred_size;
        }

        None
    }

    pub(crate) fn requested_initial_window_size(
        &self,
        surface: &ToplevelSurface,
        properties: &WindowProperties,
    ) -> Option<Size> {
        self.requested_initial_window_size_from(properties, Self::client_preferred_size(surface))
    }

    pub(crate) fn remember_window_size_for_id(&mut self, id: WindowId) {
        if !self.sizing_config().remember_sizes_by_app_id {
            return;
        }
        let Some(window) = self.window_models.get(&id) else {
            return;
        };
        let Some(key) = Self::size_memory_key(&window.properties) else {
            return;
        };
        self.remembered_app_sizes.insert(key, window.bounds.size);
    }

    pub(crate) fn apply_pending_client_default_size(&mut self, root_surface: &WlSurface) {
        let Some(id) = self.window_id_for_surface(root_surface) else {
            return;
        };
        if !self.pending_client_default_size.contains(&id) {
            return;
        }

        let Some(window) = self
            .space
            .elements()
            .find(|window| window_matches_surface(window, root_surface))
            .cloned()
        else {
            return;
        };
        let geometry = window.geometry();
        if geometry.size.w <= 0 || geometry.size.h <= 0 {
            return;
        }

        if let Some(model) = self.window_models.get_mut(&id) {
            model.bounds.size = Size::new(geometry.size.w as f64, geometry.size.h as f64);
        }
        self.pending_client_default_size.remove(&id);
        self.remember_window_size_for_id(id);
        self.request_redraw();
    }

    pub(crate) fn placement_rect_for(&self, requested_size: Option<Size>) -> Rect {
        let existing = self.window_models.values().cloned().collect::<Vec<_>>();
        let mut bounds = self
            .fallback_placement_policy
            .place_new_window(self.viewport(), &existing, requested_size)
            .bounds;

        if !self.is_tty_backend()
            && let Some(output) = self.space.outputs().next()
        {
            let zone = layer_map_for_output(output).non_exclusive_zone();
            if zone.size.w > 0 && zone.size.h > 0 {
                bounds.origin.x = bounds
                    .origin
                    .x
                    .clamp(zone.loc.x as f64, (zone.loc.x + zone.size.w) as f64);
                bounds.origin.y = bounds
                    .origin
                    .y
                    .clamp(zone.loc.y as f64, (zone.loc.y + zone.size.h) as f64);
            }
        }

        bounds
    }

    pub(crate) fn apply_rules_to_window(window: &mut WindowModel, applied_rules: &AppliedWindowRules) {
        if let Some(floating) = applied_rules.floating {
            window.floating = floating;
        }
        if let Some(exclude_from_focus) = applied_rules.exclude_from_focus {
            window.exclude_from_focus = exclude_from_focus;
        }
    }

    /// Sync WindowModel properties from the Wayland toplevel surface.
    ///
    /// Called during initial map (from `new_toplevel`) and on subsequent
    /// commits that update app_id or title.
    ///
    /// Ordering:
    /// 1. Read current properties from the toplevel surface
    /// 2. Update the WindowModel's app_id and title
    /// 3. Re-apply window rules (rules see the *new* properties)
    /// 4. If this is the initial configure and rules specify a default size,
    ///    apply it and send a configure to the client
    /// 5. Fire `window_property_changed` hooks for any properties that
    ///    actually changed (title, app_id) — hooks see the *post-sync*
    ///    canonical state
    pub(crate) fn sync_window_from_toplevel(
        &mut self,
        surface: &ToplevelSurface,
        send_configure_for_initial_size: bool,
    ) -> Option<WindowId> {
        let id = self.window_id_for_surface(surface.wl_surface())?;
        let new_properties = window_properties_from_toplevel(surface);

        let (default_size, size_changed, old_app_id, old_title) = {
            let window = self.window_models.get_mut(&id)?;
            let old_app_id = window.properties.app_id.clone();
            let old_title = window.properties.title.clone();
            window.properties.app_id = new_properties.app_id.clone();
            window.properties.title = new_properties.title.clone();

            let applied_rules =
                AppliedWindowRules::from_rules(&window.properties, &self.window_rules);
            Self::apply_rules_to_window(window, &applied_rules);

            let default_size = applied_rules.default_size;
            let size_changed = default_size.is_some_and(|size| {
                !surface.is_initial_configure_sent() && window.bounds.size != size
            });
            if let Some(size) = default_size
                && size_changed
            {
                window.bounds.size = size;
            }

            (default_size, size_changed, old_app_id, old_title)
        };

        if send_configure_for_initial_size
            && size_changed
            && let Some(size) = default_size
        {
            surface.with_pending_state(|state| {
                state.size = Some((size.w.round() as i32, size.h.round() as i32).into());
            });
            let _ = surface.send_pending_configure();
        }

        // Fire property-change hooks for title and app_id if they changed.
        #[cfg(feature = "lua")]
        {
            use crate::lua::PropertyValue;
            if old_title != new_properties.title {
                self.trigger_live_window_property_changed(
                    id,
                    "title",
                    PropertyValue::OptionString(old_title),
                    PropertyValue::OptionString(new_properties.title.clone()),
                );
            }
            if old_app_id != new_properties.app_id {
                self.trigger_live_window_property_changed(
                    id,
                    "app_id",
                    PropertyValue::OptionString(old_app_id),
                    PropertyValue::OptionString(new_properties.app_id.clone()),
                );
            }
        }

        self.remember_window_size_for_id(id);
        self.request_redraw();
        Some(id)
    }

    pub(crate) fn track_new_window(
        &mut self,
        surface: WlSurface,
        bounds: Rect,
        properties: WindowProperties,
    ) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;

        let mut window = WindowModel::new(id, bounds).with_properties(properties);
        let applied_rules = AppliedWindowRules::from_rules(&window.properties, &self.window_rules);
        Self::apply_rules_to_window(&mut window, &applied_rules);

        self.window_models.insert(id, window.clone());
        self.surface_window_ids.insert(surface.clone(), id);
        self.window_surfaces.insert(id, surface);
        self.emit_event(
            "window_tracked",
            serde_json::json!({
                "id": id.0,
                "app_id": window.properties.app_id,
                "title": window.properties.title,
                "bounds": {
                    "x": window.bounds.origin.x,
                    "y": window.bounds.origin.y,
                    "w": window.bounds.size.w,
                    "h": window.bounds.size.h,
                },
            }),
        );
        id
    }

    pub(crate) fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.surface_window_ids.get(surface).copied()
    }

    pub(crate) fn window_snapshot_for_id(&self, id: WindowId) -> Option<WindowSnapshot> {
        let window = self.window_models.get(&id)?;
        let output_id = self.output_id_for_window_bounds(window.bounds);
        Some(WindowSnapshot {
            id: window.id.0,
            app_id: window.properties.app_id.clone(),
            title: window.properties.title.clone(),
            bounds: window.bounds,
            floating: window.floating,
            exclude_from_focus: window.exclude_from_focus,
            focused: self.focus_stack.focused() == Some(window.id),
            fullscreen: window.fullscreen,
            maximized: window.maximized,
            urgent: window.urgent,
            mapped: true,
            mapped_at: window
                .mapped_at
                .and_then(crate::headless::system_time_to_epoch_secs),
            last_focused_at: window
                .last_focused_at
                .and_then(crate::headless::system_time_to_epoch_secs),
            output_id,
            pid: window.properties.pid,
        })
    }

    /// Return the ID of the output whose visible world contains the center of `bounds`.
    ///
    /// This is the compositor's current window-to-output association rule.
    /// Returns `None` if the window center is not inside any output's visible world.
    pub(crate) fn output_id_for_window_bounds(&self, bounds: crate::canvas::Rect) -> Option<String> {
        let center_x = bounds.origin.x + bounds.size.w / 2.0;
        let center_y = bounds.origin.y + bounds.size.h / 2.0;

        let in_rect = |vw: crate::canvas::Rect| -> bool {
            center_x >= vw.origin.x
                && center_x < vw.origin.x + vw.size.w
                && center_y >= vw.origin.y
                && center_y < vw.origin.y + vw.size.h
        };

        if self.output_states.is_empty() {
            let vw = self.output_state.viewport().visible_world_rect();
            if in_rect(vw) {
                return Some(self.output_state.name().to_string());
            }
        } else {
            for output in self.space.outputs() {
                if let Some(state) = self.output_state_for_output(output) {
                    let vw = state.viewport().visible_world_rect();
                    if in_rect(vw) {
                        return Some(output.name());
                    }
                }
            }
        }
        None
    }

    pub(crate) fn try_live_resolve_focus(&mut self, request: ResolveFocusRequest<'_>) -> bool {
        #[cfg(feature = "lua")]
        if let Some(result) =
            self.with_live_lua(|hooks, state| hooks.trigger_resolve_focus(state, request))
        {
            return match result {
                Ok(handled) => handled,
                Err(error) => {
                    eprintln!("{}", format_live_hook_error("resolve_focus", &error));
                    false
                }
            };
        }

        false
    }

    /// Live close/unmap/destroy lifecycle when a surface is destroyed.
    ///
    /// Ordering:
    /// 1. Cancel any active interactive operation (move/resize) on the window
    /// 2. Remove surface→ID and ID→surface mappings
    /// 3. Remove the WindowModel and remove from FocusStack (no auto-fallback)
    /// 4. Build a final WindowSnapshot from the removed model (captures
    ///    pre-removal state like `focused`, `bounds`, `output_id`)
    /// 5. `trigger_live_window_unmapped` — Lua `window_unmapped` hook fires
    ///    with the final snapshot; model is already removed so `evil.state()`
    ///    reflects the post-removal world
    /// 6. `try_live_resolve_focus("window_unmapped")` — Lua `resolve_focus`
    ///    hook decides what to focus after the window is gone
    /// 7. `sync_focus_to_stack` — apply the focus decision to Wayland state
    ///    (keyboard focus, activated flag, raise)
    ///
    /// Note: `close_window()` only sends a close *request* to the client.
    /// The actual cleanup happens here when the client destroys its surface.
    pub(crate) fn untrack_surface(&mut self, surface: &WlSurface) {
        let removed = self
            .window_id_for_surface(surface)
            .and_then(|id| self.window_models.get(&id).cloned())
            .into_iter()
            .collect::<Vec<_>>();
        let previous_focus = self.focus_stack.focused();

        // Cancel any active interactive operation on the destroyed window.
        if let Some(active) = &self.active_interactive_op
            && removed.iter().any(|window| window.id == active.window_id)
        {
            self.active_interactive_op = None;
        }

        if let Some(id) = self.surface_window_ids.remove(surface) {
            self.window_surfaces.remove(&id);
        }
        for window in &removed {
            self.window_models.remove(&window.id);
            self.focus_stack.remove_without_fallback(window.id);
        }

        let removed_snapshots = removed
            .into_iter()
            .map(|window| {
                let focused = previous_focus == Some(window.id);
                let output_id = self.output_id_for_window_bounds(window.bounds);
                WindowSnapshot {
                    id: window.id.0,
                    app_id: window.properties.app_id.clone(),
                    title: window.properties.title.clone(),
                    bounds: window.bounds,
                    floating: window.floating,
                    exclude_from_focus: window.exclude_from_focus,
                    focused,
                    fullscreen: window.fullscreen,
                    maximized: window.maximized,
                    urgent: window.urgent,
                    mapped: true,
                    mapped_at: window
                        .mapped_at
                        .and_then(crate::headless::system_time_to_epoch_secs),
                    last_focused_at: window
                        .last_focused_at
                        .and_then(crate::headless::system_time_to_epoch_secs),
                    output_id,
                    pid: window.properties.pid,
                }
            })
            .collect::<Vec<_>>();

        let snapshot = self.state_snapshot();
        for window_snapshot in &removed_snapshots {
            self.emit_event(
                "window_unmapped",
                serde_json::json!({
                    "id": window_snapshot.id,
                    "app_id": window_snapshot.app_id,
                    "title": window_snapshot.title,
                    "bounds": {
                        "x": window_snapshot.bounds.origin.x,
                        "y": window_snapshot.bounds.origin.y,
                        "w": window_snapshot.bounds.size.w,
                        "h": window_snapshot.bounds.size.h,
                    },
                }),
            );
            self.trigger_live_window_unmapped(snapshot.clone(), window_snapshot.clone());
        }

        let _ = removed_snapshots.last().cloned().map(|window_snapshot| {
            self.try_live_resolve_focus(ResolveFocusRequest {
                reason: "window_unmapped",
                window: Some(&window_snapshot),
                previous: previous_focus,
                pointer: None,
                button: None,
                pressed: None,
                modifiers: None,
            })
        });

        self.request_redraw();
        self.sync_focus_to_stack();
    }

    pub(crate) fn cleanup_window_bookkeeping(&mut self) {
        self.surface_window_ids
            .retain(|surface, id| surface.is_alive() && self.window_models.contains_key(id));

        let live_ids = self
            .surface_window_ids
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        self.window_models.retain(|id, _| live_ids.contains(id));
        self.window_surfaces.clear();
        for (surface, id) in &self.surface_window_ids {
            self.window_surfaces.insert(*id, surface.clone());
        }

        let stale_focus_ids = self
            .focus_stack
            .order()
            .iter()
            .copied()
            .filter(|id| !live_ids.contains(id))
            .collect::<Vec<_>>();
        for id in stale_focus_ids {
            self.focus_stack.remove_without_fallback(id);
        }
    }

    pub(crate) fn find_space_window(&self, id: WindowId) -> Option<Window> {
        let surface = self.window_surfaces.get(&id)?;
        self.space
            .elements()
            .find(|window| window_matches_surface(window, surface))
            .cloned()
    }
}
