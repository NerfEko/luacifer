use super::*;

impl EvilWm {
    #[cfg(feature = "xwayland")]
    fn find_space_x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| {
                window
                    .x11_surface()
                    .is_some_and(|candidate| candidate.window_id() == surface.window_id())
            })
            .cloned()
    }

    #[cfg(feature = "xwayland")]
    fn sync_x11_window_geometry(
        &mut self,
        surface: &X11Surface,
        geometry: Rectangle<i32, Logical>,
    ) {
        if let Some(space_window) = self.find_space_x11_window(surface) {
            self.space.map_element(space_window, geometry.loc, false);
        }

        if let Some(wl_surface) = surface.wl_surface()
            && let Some(id) = self.window_id_for_surface(&wl_surface)
            && let Some(window) = self.window_models.get_mut(&id)
        {
            window.bounds.origin = Point::new(geometry.loc.x as f64, geometry.loc.y as f64);
            window.bounds.size =
                Size::new(geometry.size.w.max(1) as f64, geometry.size.h.max(1) as f64);
        }
    }

    #[cfg(feature = "xwayland")]
    fn track_mapped_x11_surface(&mut self, surface: &X11Surface, wl_surface: WlSurface) {
        if self.window_id_for_surface(&wl_surface).is_some() {
            self.sync_x11_window_geometry(surface, surface.geometry());
            return;
        }

        let properties = WindowProperties {
            app_id: Some(surface.class()),
            title: Some(surface.title()),
            pid: None,
        };
        let previous_focus = self.focus_stack.focused();
        let bounds = if let Some(space_window) = self.find_space_x11_window(surface) {
            let geometry = space_window.geometry();
            let location = self
                .space
                .element_location(&space_window)
                .unwrap_or_default();
            Rect::new(
                location.x as f64,
                location.y as f64,
                geometry.size.w.max(1) as f64,
                geometry.size.h.max(1) as f64,
            )
        } else {
            let geometry = surface.geometry();
            Rect::new(
                geometry.loc.x as f64,
                geometry.loc.y as f64,
                geometry.size.w.max(1) as f64,
                geometry.size.h.max(1) as f64,
            )
        };
        let id = self.track_new_window(wl_surface, bounds, properties);
        self.emit_event(
            "x11_window_mapped",
            serde_json::json!({
                "id": id.0,
                "app_id": self.window_models.get(&id).and_then(|window| window.properties.app_id.clone()),
                "title": self.window_models.get(&id).and_then(|window| window.properties.title.clone()),
                "x": bounds.origin.x,
                "y": bounds.origin.y,
                "w": bounds.size.w,
                "h": bounds.size.h,
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
        self.trigger_live_window_mapped(id);
        self.request_redraw();
    }
}

#[cfg(feature = "xwayland")]
impl XwmHandler for EvilWm {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.x11_wm.as_mut().expect("X11 WM not started")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(error) = window.set_mapped(true) {
            eprintln!("failed to map X11 window: {error}");
            return;
        }

        let properties = WindowProperties {
            app_id: Some(window.class()),
            title: Some(window.title()),
            pid: None,
        };
        let geometry = window.geometry();
        let client_preferred_size = (geometry.size.w > 0 && geometry.size.h > 0)
            .then(|| Size::new(geometry.size.w as f64, geometry.size.h as f64));
        let bounds = self.placement_rect_for(
            self.requested_initial_window_size_from(&properties, client_preferred_size),
        );
        let location = (
            bounds.origin.x.round() as i32,
            bounds.origin.y.round() as i32,
        );
        let desktop_window = Window::new_x11_window(window.clone());
        self.space.map_element(desktop_window, location, false);

        if let Some(wl_surface) = window.wl_surface() {
            self.track_mapped_x11_surface(&window, wl_surface);
        } else {
            self.pending_x11_windows.insert(window.window_id());
        }
        self.request_redraw();
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let geometry = window.geometry();
        self.emit_event(
            "x11_override_redirect_mapped",
            serde_json::json!({
                "window_id": window.window_id(),
                "title": window.title(),
                "class": window.class(),
                "x": geometry.loc.x,
                "y": geometry.loc.y,
                "w": geometry.size.w,
                "h": geometry.size.h,
            }),
        );
        let desktop_window = Window::new_x11_window(window.clone());
        self.space.map_element(desktop_window, geometry.loc, false);
        self.sync_x11_window_geometry(&window, geometry);
        self.request_redraw();
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.pending_x11_windows.remove(&window.window_id());
        self.emit_event(
            "x11_window_unmapped",
            serde_json::json!({
                "window_id": window.window_id(),
                "title": window.title(),
                "class": window.class(),
            }),
        );
        if let Some(wl_surface) = window.wl_surface() {
            self.untrack_surface(&wl_surface);
        }
        if let Some(space_window) = self.find_space_x11_window(&window) {
            self.space.unmap_elem(&space_window);
        }
        self.request_redraw();
    }

    fn destroyed_window(&mut self, xwm: XwmId, window: X11Surface) {
        self.unmapped_window(xwm, window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<smithay::xwayland::xwm::Reorder>,
    ) {
        self.emit_event(
            "x11_configure_request",
            serde_json::json!({
                "window_id": window.window_id(),
                "x": x,
                "y": y,
                "w": w,
                "h": h,
                "title": window.title(),
                "class": window.class(),
            }),
        );
        let mut geometry = window.geometry();
        if let Some(x) = x {
            geometry.loc.x = x;
        }
        if let Some(y) = y {
            geometry.loc.y = y;
        }
        if let Some(w) = w {
            geometry.size.w = w as i32;
        }
        if let Some(h) = h {
            geometry.size.h = h as i32;
        }
        let _ = window.configure(geometry);
        self.sync_x11_window_geometry(&window, geometry);
        self.request_redraw();
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<X11Window>,
    ) {
        self.emit_event(
            "x11_configure_notify",
            serde_json::json!({
                "window_id": window.window_id(),
                "x": geometry.loc.x,
                "y": geometry.loc.y,
                "w": geometry.size.w,
                "h": geometry.size.h,
                "title": window.title(),
                "class": window.class(),
            }),
        );
        self.sync_x11_window_geometry(&window, geometry);
        self.request_redraw();
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: smithay::xwayland::xwm::ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}
}

#[cfg(feature = "xwayland")]
impl XWaylandShellHandler for EvilWm {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, wl_surface: WlSurface, surface: X11Surface) {
        if self.pending_x11_windows.remove(&surface.window_id())
            || self.window_id_for_surface(&wl_surface).is_none()
        {
            self.track_mapped_x11_surface(&surface, wl_surface);
        }
    }
}

#[cfg(feature = "xwayland")]
pub(crate) fn spawn_xwayland(
    display_handle: &DisplayHandle,
    loop_handle: &smithay::reexports::calloop::LoopHandle<'static, EvilWm>,
) {
    use std::process::Stdio;

    let (xwayland, client) = match XWayland::spawn(
        display_handle,
        None,
        std::iter::empty::<(String, String)>(),
        true,
        Stdio::null(),
        Stdio::null(),
        |_| (),
    ) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("failed to spawn XWayland: {error}");
            return;
        }
    };

    let handle = loop_handle.clone();
    if let Err(error) = loop_handle.insert_source(xwayland, move |event, _, state| match event {
        XWaylandEvent::Ready {
            x11_socket,
            display_number,
        } => {
            unsafe {
                std::env::set_var("DISPLAY", format!(":{display_number}"));
            }
            match X11Wm::start_wm(handle.clone(), x11_socket, client.clone()) {
                Ok(wm) => {
                    println!("XWayland ready on DISPLAY=:{display_number}");
                    state.emit_event(
                        "xwayland_ready",
                        serde_json::json!({
                            "display": format!(":{display_number}"),
                        }),
                    );
                    state.x11_wm = Some(wm);
                }
                Err(error) => eprintln!("failed to start X11 WM: {error}"),
            }
        }
        XWaylandEvent::Error => {
            eprintln!("XWayland exited unexpectedly");
            state.emit_event("xwayland_exited", serde_json::json!({}));
            state.x11_wm = None;
        }
    }) {
        eprintln!("failed to register XWayland event source: {error}");
    }
}
