use super::*;
use smithay::{delegate_compositor, delegate_data_device, delegate_idle_inhibit, delegate_layer_shell, delegate_output, delegate_primary_selection, delegate_seat, delegate_shm, delegate_xdg_shell};
#[cfg(feature = "xwayland")]
use smithay::delegate_xwayland_shell;

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl CompositorHandler for EvilWm {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<ClientState>() {
            return &data.compositor_state;
        }
        #[cfg(feature = "xwayland")]
        if let Some(data) = client.get_data::<XWaylandClientData>() {
            return &data.compositor_state;
        }
        panic!("missing compositor client state for wayland client");
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let committed_window = self
                .space
                .elements()
                .find(|window| window_matches_surface(window, &root))
                .cloned();
            if let Some(window) = committed_window {
                window.on_commit();
                self.apply_pending_client_default_size(&root);
            }
        }

        handle_commit(&mut self.popups, &self.space, surface);
        self.request_redraw();
    }

    fn destroyed(&mut self, surface: &WlSurface) {
        self.untrack_surface(surface);
        self.cleanup_window_bookkeeping();
        self.request_redraw();
    }
}

impl BufferHandler for EvilWm {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for EvilWm {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for EvilWm {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    /// Live map lifecycle for a new xdg-toplevel surface.
    ///
    /// Ordering:
    /// 1. `track_new_window` — allocate ID, create WindowModel, apply rules
    /// 2. `sync_window_from_toplevel` — sync properties (app_id, title) from
    ///    the Wayland surface; fire `window_property_changed` hooks if values
    ///    differ from the initial empty state; re-apply rules after sync
    /// 3. `trigger_live_place_window` — Lua `place_window` hook may override
    ///    initial bounds (rules have already been applied)
    /// 4. `space.map_element` — add the smithay desktop Window to the Space
    /// 5. `try_live_resolve_focus("window_mapped")` — Lua `resolve_focus`
    ///    hook decides whether to focus the new window
    /// 6. `surface.send_configure` — tell the client its size and activated
    ///    state (reflects focus decision from step 5)
    /// 7. `trigger_live_window_mapped` — Lua `window_mapped` hook fires;
    ///    the window is fully mapped and focused (or not) by this point
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let properties = window_properties_from_toplevel(&surface);
        let requested_size = self.requested_initial_window_size(&surface, &properties);
        let should_wait_for_client_size =
            requested_size.is_none() && self.sizing_config().use_client_default_size;
        let fallback_rect = self.placement_rect_for(requested_size);
        let previous_focus = self.focus_stack.focused();
        let id = self.track_new_window(surface.wl_surface().clone(), fallback_rect, properties);
        if should_wait_for_client_size {
            self.pending_client_default_size.insert(id);
        }
        let _ = self.sync_window_from_toplevel(&surface, false);
        self.trigger_live_place_window(id);

        let configured_rect = self
            .window_models
            .get(&id)
            .map(|window| window.bounds)
            .unwrap_or(fallback_rect);
        let location = (
            configured_rect.origin.x.round() as i32,
            configured_rect.origin.y.round() as i32,
        );
        let window = Window::new_wayland_window(surface.clone());
        self.space.map_element(window, location, false);
        self.emit_event(
            "wayland_window_mapped",
            serde_json::json!({
                "id": id.0,
                "x": configured_rect.origin.x,
                "y": configured_rect.origin.y,
                "w": configured_rect.size.w,
                "h": configured_rect.size.h,
            }),
        );

        let mapped_snapshot = self.window_snapshot_for_id(id);
        let _ = self.try_live_resolve_focus(ResolveFocusRequest {
            reason: "window_mapped",
            window: mapped_snapshot.as_ref(),
            previous: previous_focus,
            pointer: None,
            button: None,
            pressed: None,
            modifiers: None,
        });

        let configured_rect = self
            .window_models
            .get(&id)
            .map(|window| window.bounds)
            .unwrap_or(configured_rect);
        surface.with_pending_state(|state| {
            if self.focus_stack.focused() == Some(id) {
                state.states.set(xdg_toplevel::State::Activated);
            }
            if requested_size.is_some() {
                state.size = Some(
                    (
                        configured_rect.size.w.round() as i32,
                        configured_rect.size.h.round() as i32,
                    )
                        .into(),
                );
            }
        });
        surface.send_configure();
        self.trigger_live_window_mapped(id);
        self.request_redraw();
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    /// Live interactive move lifecycle.
    ///
    /// Begin: client calls xdg_toplevel.move → create ActiveInteractiveOp →
    ///        `trigger_live_move_begin`
    /// Update: pointer motion → `active.advance()` computes delta →
    ///         `trigger_live_move_update` (Lua applies the delta)
    /// End:   initiating button released → `trigger_live_move_end` →
    ///        op cleared
    ///
    /// If the window is destroyed mid-move, `untrack_surface` cancels the op.
    /// If the window model disappears during an update, the op is also cancelled.
    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
        let _ = self.begin_interactive_move(id);
    }

    /// Live interactive resize lifecycle (same pattern as move_request).
    ///
    /// Begin: client calls xdg_toplevel.resize → create ActiveInteractiveOp
    ///        with resize_edges → `trigger_live_resize_begin`
    /// Update: pointer motion → `trigger_live_resize_update` (delta + edges)
    /// End:   initiating button released → `trigger_live_resize_end` → op cleared
    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: wl_seat::WlSeat,
        _serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
        let _ = self.begin_interactive_resize(id, resize_edges_from(edges));
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let _ = self.sync_window_from_toplevel(&surface, true);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let _ = self.sync_window_from_toplevel(&surface, true);
    }
}

impl WlrLayerShellHandler for EvilWm {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<WlOutput>,
        layer: WlrLayer,
        namespace: String,
    ) {
        let Some(resolved_output) = self.resolve_layer_output(output.as_ref()) else {
            eprintln!("dropping layer surface {namespace}: no output available");
            return;
        };

        let desktop_surface = DesktopLayerSurface::new(surface, namespace.clone());
        if matches!(layer, WlrLayer::Top | WlrLayer::Bottom)
            && let Some(output_geo) = self.space.output_geometry(&resolved_output)
        {
            desktop_surface.layer_surface().with_pending_state(|state| {
                let current = state.size.unwrap_or_else(|| (0, 0).into());
                if current.w == 0 {
                    state.size = Some((output_geo.size.w, current.h).into());
                }
            });
        }

        let mut map = layer_map_for_output(&resolved_output);
        if let Err(error) = map.map_layer(&desktop_surface) {
            eprintln!("failed to map layer surface {namespace}: {error}");
            return;
        }

        println!(
            "mapped layer surface namespace={} layer={:?} output={}",
            namespace,
            layer,
            resolved_output.name()
        );
        self.emit_event(
            "layer_surface_mapped",
            serde_json::json!({
                "namespace": namespace,
                "layer": format!("{:?}", layer),
                "output": resolved_output.name(),
            }),
        );
        self.request_redraw();
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(popup)) {
            eprintln!("failed to track layer popup: {error}");
        }
        self.request_redraw();
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        for output in self.space.outputs().cloned().collect::<Vec<_>>() {
            let layer = {
                let map = layer_map_for_output(&output);
                map.layer_for_surface(surface.wl_surface(), WindowSurfaceType::TOPLEVEL)
                    .cloned()
            };
            if let Some(layer) = layer {
                println!(
                    "unmapped layer surface namespace={} output={}",
                    layer.namespace(),
                    output.name()
                );
                self.emit_event(
                    "layer_surface_unmapped",
                    serde_json::json!({
                        "namespace": layer.namespace(),
                        "output": output.name(),
                    }),
                );
                layer_map_for_output(&output).unmap_layer(&layer);
                break;
            }
        }
        self.request_redraw();
    }
}

pub(super) fn window_toplevel(window: &Window) -> Option<ToplevelSurface> {
    window.toplevel().cloned()
}

pub(super) fn window_matches_surface(window: &Window, surface: &WlSurface) -> bool {
    if window_toplevel(window).is_some_and(|toplevel| toplevel.wl_surface() == surface) {
        return true;
    }

    #[cfg(feature = "xwayland")]
    if window
        .x11_surface()
        .and_then(|x11| x11.wl_surface())
        .as_ref()
        .is_some_and(|candidate| candidate == surface)
    {
        return true;
    }

    false
}

impl EvilWm {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let popup_kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&popup_kind) else {
            return;
        };

        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };

        let target = if let Some(window) = self
            .space
            .elements()
            .find(|window| window_matches_surface(window, &root))
        {
            let Some(window_geo) = self.space.element_geometry(window) else {
                return;
            };
            let mut target = output_geo;
            target.loc -= get_popup_toplevel_coords(&popup_kind);
            target.loc -= window_geo.loc;
            target
        } else {
            let map = layer_map_for_output(output);
            let Some(layer_geo) = map
                .layers()
                .find(|layer| layer.wl_surface() == &root)
                .and_then(|layer| map.layer_geometry(layer))
            else {
                return;
            };
            let mut target = Rectangle::from_size(output_geo.size);
            target.loc -= get_popup_toplevel_coords(&popup_kind);
            target.loc -= layer_geo.loc;
            target
        };

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub(crate) fn arrange_layer_surfaces_for_output(&mut self, output: &Output) {
        let changed = layer_map_for_output(output).arrange();
        if changed {
            self.request_redraw();
        }
    }

    fn resolve_layer_output(&self, requested: Option<&WlOutput>) -> Option<Output> {
        requested
            .and_then(|wl_output| {
                let client = wl_output.client()?;
                self.space
                    .outputs()
                    .find(|output| {
                        output
                            .client_outputs(&client)
                            .any(|candidate| candidate == *wl_output)
                    })
                    .cloned()
            })
            .or_else(|| self.space.outputs().next().cloned())
    }

    pub(crate) fn idle_inhibited(&self) -> bool {
        self.idle_inhibitors.iter().any(|surface| {
            self.surface_window_ids.contains_key(surface)
                || self
                    .space
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
        })
    }

    pub(crate) fn session_locked(&self) -> bool {
        self.session_locked
    }

    pub(crate) fn set_session_locked(&mut self, locked: bool) {
        self.session_locked = locked;
        self.emit_event(
            "session_lock_changed",
            serde_json::json!({
                "locked": locked,
            }),
        );
        self.request_redraw();
    }

    #[cfg(feature = "lua")]
    pub(crate) fn record_live_hook_error(&mut self, hook_name: &str, error: &crate::lua::ConfigError) {
        let formatted = format_live_hook_error(hook_name, error);
        let entry = self
            .live_hook_errors
            .entry(hook_name.to_string())
            .or_insert_with(|| LiveHookErrorState {
                count: 0,
                last_error: formatted.clone(),
            });
        entry.count += 1;
        let count = entry.count;
        let changed_message = entry.last_error != formatted;
        entry.last_error = formatted.clone();
        if count == 1 || changed_message || count.is_multiple_of(10) {
            eprintln!("{formatted}");
        }
        self.emit_event(
            "hook_error",
            serde_json::json!({
                "hook": hook_name,
                "count": count,
                "error": formatted,
            }),
        );
    }

    #[cfg(feature = "lua")]
    pub(crate) fn live_hook_errors_snapshot(&self) -> Vec<crate::ipc::HookErrorSnapshot> {
        self.live_hook_errors
            .iter()
            .map(|(hook, state)| crate::ipc::HookErrorSnapshot {
                hook: hook.clone(),
                count: state.count,
                last_error: state.last_error.clone(),
            })
            .collect()
    }
}

fn handle_commit(popups: &mut PopupManager, space: &Space<Window>, surface: &WlSurface) {
    if let Some(window) = space
        .elements()
        .find(|window| window_matches_surface(window, surface))
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok().map(|guard| guard.initial_configure_sent))
                .unwrap_or(false)
        });

        if !initial_configure_sent && let Some(toplevel) = window_toplevel(&window) {
            toplevel.send_configure();
        }
    }

    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface)
        && let PopupKind::Xdg(ref xdg) = popup
        && !xdg.is_initial_configure_sent()
        && let Err(error) = xdg.send_configure()
    {
        eprintln!("initial popup configure failed: {error}");
    }
}

impl SeatHandler for EvilWm {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<EvilWm> {
        &mut self.seat_state
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
    }
}

impl SelectionHandler for EvilWm {
    type SelectionUserData = ();
}

impl PrimarySelectionHandler for EvilWm {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl IdleInhibitHandler for EvilWm {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.insert(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.remove(&surface);
    }
}

impl DataDeviceHandler for EvilWm {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for EvilWm {}

impl ServerDndGrabHandler for EvilWm {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

impl OutputProtocolHandler for EvilWm {
    fn output_management_state(&mut self) -> &mut OutputProtocolState {
        &mut self.output_management_protocol_state
    }

    fn apply_output_config(
        &mut self,
        _configs: Vec<crate::output_management_protocol::RequestedHeadConfig>,
    ) -> bool {
        false
    }
}

impl OutputHandler for EvilWm {}

delegate_xdg_shell!(EvilWm);
delegate_layer_shell!(EvilWm);
#[cfg(feature = "xwayland")]
delegate_xwayland_shell!(EvilWm);
delegate_compositor!(EvilWm);
delegate_shm!(EvilWm);
delegate_seat!(EvilWm);
delegate_data_device!(EvilWm);
delegate_primary_selection!(EvilWm);
delegate_idle_inhibit!(EvilWm);
crate::delegate_output_management!(EvilWm);
delegate_output!(EvilWm);
