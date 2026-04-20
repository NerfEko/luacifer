use super::*;
use crate::{
    canvas::{Point, Size},
    lua::{
        DrawConfig, OutputSnapshot, PointerSnapshot, RuntimeStateSnapshot, ViewportSnapshot,
        WindowSnapshot,
    },
    output::OutputState,
    output_management_protocol::{
        ModeInfo, OutputHeadState, notify_changes as notify_output_management_changes,
    },
};

impl EvilWm {
    pub(crate) fn primary_output_name(&self) -> Option<String> {
        self.space
            .outputs()
            .next()
            .map(|output| output.name())
            .or_else(|| self.output_states.keys().next().cloned())
    }

    pub(crate) fn output_state_for_name(&self, name: &str) -> Option<&OutputState> {
        self.output_states.get(name)
    }

    pub(crate) fn output_state_for_name_mut(&mut self, name: &str) -> Option<&mut OutputState> {
        self.output_states.get_mut(name)
    }

    pub(crate) fn output_state_for_output(&self, output: &Output) -> Option<&OutputState> {
        self.output_state_for_name(&output.name())
    }

    pub(crate) fn register_output_state(
        &mut self,
        name: impl Into<String>,
        logical_position: Point,
        screen_size: Size,
    ) {
        let name = name.into();
        let mut viewport = self
            .primary_output_name()
            .and_then(|primary| {
                self.output_states
                    .get(&primary)
                    .map(|state| state.viewport().clone())
            })
            .unwrap_or_else(|| self.output_state.viewport().clone());
        viewport.set_screen_size(screen_size);

        self.output_states.insert(
            name.clone(),
            OutputState::with_viewport(name.clone(), logical_position, viewport),
        );
        self.emit_event(
            "output_registered",
            serde_json::json!({
                "name": name,
                "logical_position": { "x": logical_position.x, "y": logical_position.y },
                "screen_size": { "w": screen_size.w, "h": screen_size.h },
            }),
        );
    }

    pub(crate) fn sync_output_state(
        &mut self,
        name: &str,
        logical_position: Point,
        screen_size: Size,
    ) {
        if let Some(state) = self.output_state_for_name_mut(name) {
            state.set_logical_position(logical_position);
            state.viewport_mut().set_screen_size(screen_size);
            self.emit_event(
                "output_updated",
                serde_json::json!({
                    "name": name,
                    "logical_position": { "x": logical_position.x, "y": logical_position.y },
                    "screen_size": { "w": screen_size.w, "h": screen_size.h },
                }),
            );
        } else {
            self.register_output_state(name.to_string(), logical_position, screen_size);
        }
    }

    pub(crate) fn notify_output_management_state(&mut self) {
        let mut heads = HashMap::new();
        for output in self.space.outputs() {
            let Some(output_state) = self.output_state_for_output(output) else {
                continue;
            };
            let physical = output.physical_properties();
            let logical_position = output_state.logical_position();
            let screen_size = output_state.viewport().screen_size();
            let refresh = output
                .current_mode()
                .map(|mode| mode.refresh)
                .unwrap_or(60_000);
            heads.insert(
                output.name(),
                OutputHeadState {
                    name: output.name(),
                    description: format!("{} {}", physical.make, physical.model),
                    make: physical.make,
                    model: physical.model,
                    serial_number: String::new(),
                    physical_size: (physical.size.w, physical.size.h),
                    modes: vec![ModeInfo {
                        width: screen_size.w.round() as i32,
                        height: screen_size.h.round() as i32,
                        refresh,
                        preferred: true,
                    }],
                    current_mode_index: Some(0),
                    position: (
                        logical_position.x.round() as i32,
                        logical_position.y.round() as i32,
                    ),
                    transform: output.current_transform(),
                    scale: output.current_scale().fractional_scale(),
                },
            );
        }

        notify_output_management_changes::<Self>(&mut self.output_management_protocol_state, heads);
    }

    #[cfg(feature = "udev")]
    pub(crate) fn remove_output_state(&mut self, name: &str) {
        self.output_states.remove(name);
    }

    pub(crate) fn clone_primary_camera_to_all_outputs(&mut self) {
        let Some(primary_name) = self.primary_output_name() else {
            return;
        };
        let Some(primary_viewport) = self
            .output_states
            .get(&primary_name)
            .map(|state| state.viewport().clone())
        else {
            return;
        };

        for (name, state) in &mut self.output_states {
            if *name == primary_name {
                continue;
            }
            let screen_size = state.viewport().screen_size();
            let mut viewport = primary_viewport.clone();
            viewport.set_screen_size(screen_size);
            state.set_viewport(viewport);
        }
    }

    pub(crate) fn pan_all_viewports(&mut self, delta: crate::canvas::Vec2) {
        if let Some(primary_name) = self.primary_output_name()
            && let Some(state) = self.output_state_for_name_mut(&primary_name)
        {
            state.viewport_mut().pan_world(delta);
            self.clone_primary_camera_to_all_outputs();
        } else {
            self.output_state.viewport_mut().pan_world(delta);
        }

        if self.is_tty_backend()
            && let Some(pointer) = self.seat.get_pointer()
        {
            let current = pointer.current_location();
            pointer.set_location((current.x + delta.x, current.y + delta.y).into());
        }
    }

    pub(crate) fn zoom_all_viewports_at_primary(&mut self, anchor: Point, factor: f64) {
        if let Some(primary_name) = self.primary_output_name()
            && let Some(state) = self.output_state_for_name_mut(&primary_name)
        {
            state.viewport_mut().zoom_at_screen(anchor, factor);
            self.clone_primary_camera_to_all_outputs();
            return;
        }

        self.output_state
            .viewport_mut()
            .zoom_at_screen(anchor, factor);
    }

    pub fn viewport(&self) -> &crate::canvas::Viewport {
        self.primary_output_name()
            .and_then(|name| self.output_states.get(&name).map(OutputState::viewport))
            .unwrap_or_else(|| self.output_state.viewport())
    }

    pub fn viewport_mut(&mut self) -> &mut crate::canvas::Viewport {
        if let Some(name) = self.primary_output_name()
            && self.output_states.contains_key(&name)
        {
            return self.output_states.get_mut(&name).unwrap().viewport_mut();
        }
        self.output_state.viewport_mut()
    }

    pub(crate) fn draw_clear_color(&self) -> [f32; 4] {
        self.config
            .as_ref()
            .map(|cfg| cfg.draw.clear_color)
            .unwrap_or(DrawConfig::default().clear_color)
    }

    pub(crate) fn canvas_allows_pointer_zoom(&self) -> bool {
        self.config
            .as_ref()
            .map(|cfg| cfg.canvas.allow_pointer_zoom)
            .unwrap_or(true)
    }

    pub(crate) fn canvas_allows_middle_click_pan(&self) -> bool {
        self.config
            .as_ref()
            .map(|cfg| cfg.canvas.allow_middle_click_pan)
            .unwrap_or(true)
    }

    pub(crate) fn canvas_allows_gesture_navigation(&self) -> bool {
        self.config
            .as_ref()
            .map(|cfg| cfg.canvas.allow_gesture_navigation)
            .unwrap_or(true)
    }

    #[cfg(feature = "udev")]
    pub(crate) fn tty_output_layout(&self) -> crate::lua::TtyOutputLayout {
        self.config
            .as_ref()
            .map(|cfg| cfg.tty.output_layout)
            .unwrap_or(crate::lua::TtyOutputLayout::Horizontal)
    }

    #[cfg(feature = "udev")]
    pub(crate) fn tty_control_action(
        &self,
        key: &str,
        modifiers: crate::input::ModifierSet,
    ) -> Option<super::TtyControlAction> {
        let default_tty = crate::lua::TtyConfig::default();
        let tty_config = self
            .config
            .as_ref()
            .map(|cfg| &cfg.tty)
            .unwrap_or(&default_tty);
        super::tty_control_action_for(key, modifiers, tty_config)
    }

    #[cfg(feature = "udev")]
    pub(crate) fn sync_output_positions_to_viewport(&mut self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();

        let mut offset_x = 0;
        let mut offset_y = 0;
        let vertical = matches!(
            self.tty_output_layout(),
            crate::lua::TtyOutputLayout::Vertical
        );
        for output in outputs {
            let location = (offset_x, offset_y);
            output.change_current_state(None, None, None, Some(location.into()));
            self.space.map_output(&output, location);
            if let Some(geometry) = self.space.output_geometry(&output) {
                self.sync_output_state(
                    &output.name(),
                    Point::new(geometry.loc.x as f64, geometry.loc.y as f64),
                    Size::new(geometry.size.w as f64, geometry.size.h as f64),
                );
                if vertical {
                    offset_y += geometry.size.h;
                } else {
                    offset_x += geometry.size.w;
                }
            }
        }
    }

    #[cfg(feature = "udev")]
    pub(crate) fn sync_primary_output_state_from_space(&mut self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        if outputs.is_empty() {
            self.output_states.clear();
            return;
        }

        let live_names = outputs
            .iter()
            .map(Output::name)
            .collect::<std::collections::BTreeSet<_>>();
        self.output_states
            .retain(|name, _| live_names.contains(name));

        for output in &outputs {
            if let Some(geometry) = self.space.output_geometry(output) {
                self.sync_output_state(
                    &output.name(),
                    Point::new(geometry.loc.x as f64, geometry.loc.y as f64),
                    Size::new(geometry.size.w as f64, geometry.size.h as f64),
                );
            }
        }
    }

    #[cfg(feature = "udev")]
    pub(crate) fn center_pointer_on_primary_output(&mut self) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let center = if self.is_tty_backend() {
            self.output_state_for_output(&output)
                .map(|state| {
                    let screen = state.viewport().screen_size();
                    state
                        .viewport()
                        .screen_to_world(Point::new(screen.w / 2.0, screen.h / 2.0))
                })
                .map(|point| (point.x, point.y).into())
        } else {
            self.space.output_geometry(&output).map(|geometry| {
                (
                    geometry.loc.x as f64 + geometry.size.w as f64 / 2.0,
                    geometry.loc.y as f64 + geometry.size.h as f64 / 2.0,
                )
                    .into()
            })
        };
        if let Some(pointer) = self.seat.get_pointer()
            && let Some(center) = center
        {
            pointer.set_location(center);
        }
    }

    pub fn state_snapshot(&self) -> RuntimeStateSnapshot {
        let focused = self.focus_stack.focused();
        let pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_else(|| (0.0, 0.0).into());

        let outputs = if self.output_states.is_empty() {
            let viewport = self.output_state.viewport();
            let logical_position = self.output_state.logical_position();
            vec![OutputSnapshot {
                id: self.output_state.name().to_string(),
                logical_x: logical_position.x,
                logical_y: logical_position.y,
                viewport: ViewportSnapshot {
                    x: viewport.world_origin().x,
                    y: viewport.world_origin().y,
                    zoom: viewport.zoom(),
                    screen_w: viewport.screen_size().w,
                    screen_h: viewport.screen_size().h,
                    visible_world: viewport.visible_world_rect(),
                },
            }]
        } else {
            self.space
                .outputs()
                .filter_map(|output| {
                    let state = self.output_state_for_output(output)?;
                    let viewport = state.viewport();
                    let logical_position = state.logical_position();
                    Some(OutputSnapshot {
                        id: output.name(),
                        logical_x: logical_position.x,
                        logical_y: logical_position.y,
                        viewport: ViewportSnapshot {
                            x: viewport.world_origin().x,
                            y: viewport.world_origin().y,
                            zoom: viewport.zoom(),
                            screen_w: viewport.screen_size().w,
                            screen_h: viewport.screen_size().h,
                            visible_world: viewport.visible_world_rect(),
                        },
                    })
                })
                .collect()
        };

        RuntimeStateSnapshot {
            focused_window_id: focused.map(|id| id.0),
            pointer: PointerSnapshot {
                x: pointer.x,
                y: pointer.y,
            },
            outputs,
            windows: self
                .window_models
                .values()
                .map(|window| {
                    let id = window.id;
                    let output_id = self.output_id_for_window_bounds(window.bounds);
                    WindowSnapshot {
                        id: id.0,
                        app_id: window.properties.app_id.clone(),
                        title: window.properties.title.clone(),
                        bounds: window.bounds,
                        floating: window.floating,
                        exclude_from_focus: window.exclude_from_focus,
                        focused: focused == Some(id),
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
                .collect(),
        }
    }
}
