use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use crate::{
    canvas::{Point, Rect, Size, Viewport},
    input::{Action, BindingMap, ModifierSet},
    lua::{
        ActionTarget, Config, ConfigError, OutputSnapshot, PointerSnapshot,
        RuntimeStateSnapshot, ViewportSnapshot, WindowSnapshot,
    },
    output::OutputState,
    window::{FocusStack, PlacementPolicy, Window, WindowId, WindowProperties},
};

#[derive(Debug, Clone)]
pub struct HeadlessOptions {
    pub config_path: Option<PathBuf>,
    pub config: Option<Config>,
    pub screen_size: Size,
}

impl Default for HeadlessOptions {
    fn default() -> Self {
        Self {
            config_path: None,
            config: None,
            screen_size: Size::new(1280.0, 720.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessSession {
    pub output_state: OutputState,
    pub focus_stack: FocusStack,
    pub fallback_placement_policy: PlacementPolicy,
    pub bindings: BindingMap,
    pub config_path: Option<PathBuf>,
    pub config: Option<Config>,
    pub next_window_id: u64,
    pub pointer_position: Point,
    pub window_models: BTreeMap<WindowId, Window>,
    pub pending_close_requests: BTreeSet<WindowId>,
    pub unmapped_windows: BTreeMap<WindowId, Window>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HeadlessReport {
    pub config_loaded: bool,
    pub screen_size: Size,
    pub zoom: f64,
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub visible_world: Rect,
    pub next_placement: Rect,
    pub bindings: usize,
    pub rules: usize,
    pub autostart: usize,
    pub config_path: Option<PathBuf>,
}

impl HeadlessSession {
    pub fn new(options: HeadlessOptions) -> Self {
        let HeadlessOptions {
            config_path,
            config,
            screen_size,
        } = options;

        let mut viewport = Viewport::new(screen_size);
        if let Some(cfg) = &config
            && let Ok(configured) =
                viewport.clone().try_with_zoom_limits(cfg.canvas.min_zoom, cfg.canvas.max_zoom)
        {
            viewport = configured;
        }

        let bindings = config
            .as_ref()
            .map(|cfg| {
                BindingMap::from_config(&cfg.bindings, cfg.canvas.pan_step, cfg.canvas.zoom_step)
            })
            .unwrap_or_default();

        Self {
            output_state: OutputState::with_viewport("headless", Point::new(0.0, 0.0), viewport),
            focus_stack: FocusStack::default(),
            fallback_placement_policy: PlacementPolicy::default(),
            bindings,
            config_path,
            config,
            next_window_id: 1,
            pointer_position: Point::new(0.0, 0.0),
            window_models: BTreeMap::new(),
            pending_close_requests: BTreeSet::new(),
            unmapped_windows: BTreeMap::new(),
        }
    }

    pub fn viewport(&self) -> &Viewport {
        self.output_state.viewport()
    }

    pub fn viewport_mut(&mut self) -> &mut Viewport {
        self.output_state.viewport_mut()
    }

    pub fn windows(&self) -> impl Iterator<Item = &Window> {
        self.window_models.values()
    }

    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.window_models.get(&id)
    }

    pub fn focused_window_id(&self) -> Option<WindowId> {
        self.focus_stack.focused()
    }

    pub fn next_placement(&self) -> Rect {
        let existing = self.window_models.values().cloned().collect::<Vec<_>>();
        self.fallback_placement_policy
            .place_new_window(self.viewport(), &existing, None)
            .bounds
    }

    pub fn create_window(&mut self, bounds: Rect, properties: WindowProperties) -> WindowId {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;

        let window = Window::new(id, bounds).with_properties(properties);
        self.window_models.insert(id, window);
        id
    }

    pub fn set_pointer_position(&mut self, position: Point) {
        self.pointer_position = position;
    }

    pub fn focus_window(&mut self, id: WindowId) -> bool {
        if self.window_models.contains_key(&id) {
            self.focus_stack.focus(id);
            true
        } else {
            false
        }
    }

    pub fn clear_focus(&mut self) -> bool {
        let changed = self.focus_stack.focused().is_some();
        self.focus_stack.clear_focus_only();
        changed
    }

    pub fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        if let Some(window) = self.window_models.get_mut(&id) {
            window.bounds.origin = Point::new(x, y);
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
            true
        } else {
            false
        }
    }

    pub fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool {
        if bounds.size.w <= 0.0 || bounds.size.h <= 0.0 {
            return false;
        }

        if let Some(window) = self.window_models.get_mut(&id) {
            window.bounds = bounds;
            true
        } else {
            false
        }
    }

    pub fn request_close_window(&mut self, id: WindowId) -> bool {
        if self.window_models.contains_key(&id) {
            self.pending_close_requests.insert(id);
            true
        } else {
            false
        }
    }

    pub fn is_close_requested(&self, id: WindowId) -> bool {
        self.pending_close_requests.contains(&id)
    }

    pub fn unmap_window(&mut self, id: WindowId) -> bool {
        let Some(window) = self.window_models.remove(&id) else {
            return false;
        };
        self.pending_close_requests.remove(&id);
        self.focus_stack.remove_without_fallback(id);
        self.unmapped_windows.insert(id, window);
        true
    }

    pub fn unmapped_window(&self, id: WindowId) -> Option<&Window> {
        self.unmapped_windows.get(&id)
    }

    pub fn destroy_window(&mut self, id: WindowId) -> bool {
        self.pending_close_requests.remove(&id);
        self.unmapped_windows.remove(&id).is_some()
    }

    pub fn close_window(&mut self, id: WindowId) -> bool {
        if !self.request_close_window(id) {
            return false;
        }
        self.unmap_window(id) && self.destroy_window(id)
    }

    pub fn apply_action(&mut self, action: Action) {
        match action {
            Action::CloseWindow => {
                if let Some(id) = self.focused_window_id() {
                    let _ = self.close_window(id);
                }
            }
            Action::Spawn { .. } => {}
            other => other.apply_to_viewport(self.viewport_mut()),
        }
    }

    pub fn trigger_binding(&mut self, key: &str, modifiers: ModifierSet) -> bool {
        if let Some(action) = self.bindings.resolve(key, modifiers) {
            self.apply_action(action);
            return true;
        }
        false
    }

    pub fn state_snapshot(&self) -> RuntimeStateSnapshot {
        let focused = self.focus_stack.focused();
        let viewport = self.viewport();
        let logical_position = self.output_state.logical_position();

        RuntimeStateSnapshot {
            focused_window_id: focused.map(|id| id.0),
            pointer: PointerSnapshot {
                x: self.pointer_position.x,
                y: self.pointer_position.y,
            },
            outputs: vec![OutputSnapshot {
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
            }],
            windows: self
                .window_models
                .values()
                .map(|window| WindowSnapshot {
                    id: window.id.0,
                    app_id: window.properties.app_id.clone(),
                    title: window.properties.title.clone(),
                    bounds: window.bounds,
                    floating: window.floating,
                    exclude_from_focus: window.exclude_from_focus,
                    focused: focused == Some(window.id),
                })
                .collect(),
        }
    }

    pub fn report(&self) -> HeadlessReport {
        let visible_world = self.viewport().visible_world_rect();
        let next_placement = self.next_placement();

        HeadlessReport {
            config_loaded: self.config.is_some(),
            screen_size: self.output_state.viewport().screen_size(),
            zoom: self.viewport().zoom(),
            min_zoom: self.config.as_ref().map_or(0.1, |cfg| cfg.canvas.min_zoom),
            max_zoom: self.config.as_ref().map_or(8.0, |cfg| cfg.canvas.max_zoom),
            visible_world,
            next_placement,
            bindings: self.config.as_ref().map_or(0, |cfg| cfg.bindings.len()),
            rules: self.config.as_ref().map_or(0, |cfg| cfg.rules.len()),
            autostart: self.config.as_ref().map_or(0, |cfg| cfg.autostart.len()),
            config_path: self.config_path.clone(),
        }
    }
}

impl std::fmt::Display for HeadlessReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "evilwm headless: config_loaded={}, screen={}x{}, zoom={:.2}, zoom_range={:.2}..{:.2}, bindings={}, rules={}, autostart={}, preview_window=({},{} {}x{})",
            self.config_loaded,
            self.screen_size.w,
            self.screen_size.h,
            self.zoom,
            self.min_zoom,
            self.max_zoom,
            self.bindings,
            self.rules,
            self.autostart,
            self.next_placement.origin.x,
            self.next_placement.origin.y,
            self.next_placement.size.w,
            self.next_placement.size.h,
        )
    }
}

impl ActionTarget for HeadlessSession {
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        Self::move_window(self, id, x, y)
    }

    fn resize_window(&mut self, id: WindowId, w: f64, h: f64) -> bool {
        Self::resize_window(self, id, w, h)
    }

    fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool {
        Self::set_window_bounds(self, id, bounds)
    }

    fn begin_interactive_move(&mut self, _id: WindowId) -> bool {
        false
    }

    fn begin_interactive_resize(&mut self, _id: WindowId, _edges: crate::window::ResizeEdges) -> bool {
        false
    }

    fn focus_window(&mut self, id: WindowId) -> bool {
        Self::focus_window(self, id)
    }

    fn clear_focus(&mut self) -> bool {
        Self::clear_focus(self)
    }

    fn close_window(&mut self, id: WindowId) -> bool {
        Self::close_window(self, id)
    }

    fn pan_canvas(&mut self, dx: f64, dy: f64) {
        self.viewport_mut().pan_world(crate::canvas::Vec2::new(dx, dy));
    }

    fn zoom_canvas(&mut self, factor: f64) -> Result<(), ConfigError> {
        if factor <= 0.0 {
            return Err(ConfigError::Validation(
                "hook action zoom_canvas requires factor > 0".into(),
            ));
        }
        let screen = self.viewport().screen_size();
        self.viewport_mut()
            .zoom_at_screen(Point::new(screen.w / 2.0, screen.h / 2.0), factor);
        Ok(())
    }
}

pub fn run_headless(options: HeadlessOptions) -> HeadlessSession {
    HeadlessSession::new(options)
}
