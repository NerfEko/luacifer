#[cfg(feature = "udev")]
mod udev;

pub use crate::headless::{HeadlessOptions, HeadlessReport, HeadlessSession, run_headless};
#[cfg(feature = "udev")]
pub use udev::run_udev;

#[cfg(feature = "udev")]
use std::{cell::RefCell, rc::Rc};
use std::{
    collections::{BTreeMap, HashMap}, error::Error, ffi::OsString, io,
    os::unix::io::OwnedFd, sync::Arc, time::Duration,
};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
            GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        renderer::{
            damage::OutputDamageTracker, gles::GlesRenderer, utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent},
    },
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    desktop::{
        PopupKind, PopupManager, Space, Window, WindowSurfaceType, find_popup_root_surface,
        get_popup_toplevel_coords,
    },
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, ModifiersState, xkb},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Client, Display, DisplayHandle, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point as SmithayPoint, Rectangle, SERIAL_COUNTER, Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

use crate::{
    canvas::{Point, Rect, Size, Viewport},
    input::{BindingMap, ModifierSet},
    lua::{
        ActionTarget, Config, ConfigError, OutputSnapshot, PointerSnapshot,
        RuntimeStateSnapshot, ViewportSnapshot, WindowSnapshot,
    },
    output::OutputState,
    window::{
        AppliedWindowRules, FocusStack, PlacementPolicy, ResizeEdges, Window as WindowModel,
        WindowId, WindowProperties, WindowRule,
    },
};

#[cfg(feature = "lua")]
use crate::lua::{DrawCommand, DrawSpace, LiveLuaHooks, ResolveFocusRequest};
#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use smithay::{
    backend::renderer::{
        ImportAll,
        element::{Id, Kind, solid::SolidColorRenderElement},
    },
    utils::Physical,
};
#[cfg(feature = "udev")]
use smithay::{
    backend::renderer::{
        element::{AsRenderElements, Wrap, utils::RescaleRenderElement},
    },
    utils::Scale,
};

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    pub command: Option<String>,
    pub config_path: Option<std::path::PathBuf>,
    pub config: Option<Config>,
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
smithay::backend::renderer::element::render_elements! {
    LiveRenderElements<R, E> where R: ImportAll;
    Space=smithay::desktop::space::SpaceRenderElements<R, E>,
    Custom=SolidColorRenderElement,
}

#[cfg(feature = "udev")]
type UdevRenderElements<R, E> = LiveRenderElements<R, E>;

#[cfg(feature = "udev")]
type TtyControlCallback = dyn FnMut(TtyControlAction);

#[cfg(feature = "udev")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TtyControlAction {
    Quit,
    SwitchVt(i32),
}

#[cfg(feature = "udev")]
fn default_tty_control_action(key: &str, modifiers: ModifierSet) -> Option<TtyControlAction> {
    if !(modifiers.ctrl && modifiers.alt) {
        return None;
    }

    if key.eq_ignore_ascii_case("BackSpace") || key.eq_ignore_ascii_case("Backspace") {
        return Some(TtyControlAction::Quit);
    }

    let vt_suffix = key.strip_prefix('F').or_else(|| key.strip_prefix('f'))?;
    let Ok(vt) = vt_suffix.parse::<i32>() else {
        return None;
    };
    if (1..=12).contains(&vt) {
        Some(TtyControlAction::SwitchVt(vt))
    } else {
        None
    }
}

pub struct EvilWm {
    pub start_time: std::time::Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<EvilWm>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
    pub output_state: OutputState,
    pub output_states: BTreeMap<String, OutputState>,
    pub focus_stack: FocusStack,
    pub fallback_placement_policy: PlacementPolicy,
    pub bindings: BindingMap,
    pub window_rules: Vec<WindowRule>,
    pub next_window_id: u64,
    pub window_models: BTreeMap<WindowId, WindowModel>,
    pub surface_window_ids: HashMap<WlSurface, WindowId>,
    pub window_surfaces: HashMap<WindowId, WlSurface>,
    pub config: Option<Config>,
    pub config_path: Option<std::path::PathBuf>,
    redraw_requested: bool,
    trackpad_pinch_scale: Option<f64>,
    #[cfg(feature = "udev")]
    tty_control: Option<Rc<RefCell<Box<TtyControlCallback>>>>,
    #[cfg(feature = "udev")]
    tty_no_scanout_warned: bool,
    #[cfg(feature = "lua")]
    pub live_lua: Option<LiveLuaHooks>,
    active_interactive_op: Option<ActiveInteractiveOp>,
    last_pointer_button_pressed: Option<u32>,
    warned_missing_move_update_hook: bool,
    warned_missing_resize_update_hook: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveInteractiveKind {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveInteractiveOp {
    kind: ActiveInteractiveKind,
    window_id: WindowId,
    last_pointer: Point,
    total_delta: crate::canvas::Vec2,
    resize_edges: Option<ResizeEdges>,
    initiating_button: Option<u32>,
}

impl ActiveInteractiveOp {
    fn new(
        kind: ActiveInteractiveKind,
        window_id: WindowId,
        pointer: Point,
        resize_edges: Option<ResizeEdges>,
        initiating_button: Option<u32>,
    ) -> Self {
        Self {
            kind,
            window_id,
            last_pointer: pointer,
            total_delta: crate::canvas::Vec2::new(0.0, 0.0),
            resize_edges,
            initiating_button,
        }
    }

    fn advance(&mut self, pointer: Point) -> crate::canvas::Vec2 {
        let delta = pointer - self.last_pointer;
        self.last_pointer = pointer;
        self.total_delta = self.total_delta + delta;
        delta
    }

    fn total_delta(&self) -> crate::canvas::Vec2 {
        self.total_delta
    }

    fn resize_edges(&self) -> Option<ResizeEdges> {
        self.resize_edges
    }

    fn should_end_on_button(&self, button: u32) -> bool {
        self.initiating_button
            .is_none_or(|initiating| initiating == button)
    }
}

fn compile_window_rules(config: Option<&Config>) -> Vec<WindowRule> {
    config
        .map(|cfg| {
            cfg.rules
                .iter()
                .map(|rule| WindowRule {
                    app_id: rule.app_id.clone(),
                    title_contains: rule.title_contains.clone(),
                    floating: rule.floating,
                    exclude_from_focus: rule.exclude_from_focus,
                    default_size: match (rule.width, rule.height) {
                        (Some(w), Some(h)) => Some(Size::new(w, h)),
                        _ => None,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn window_properties_from_toplevel(surface: &ToplevelSurface) -> WindowProperties {
    with_states(surface.wl_surface(), |states| {
        let attributes = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();

        WindowProperties {
            app_id: attributes.app_id.clone(),
            title: attributes.title.clone(),
        }
    })
}

impl EvilWm {
    pub fn new(
        event_loop: &mut EventLoop<Self>,
        display: Display<Self>,
        config_path: Option<std::path::PathBuf>,
        config: Option<Config>,
    ) -> Result<Self, Box<dyn Error>> {
        let start_time = std::time::Instant::now();
        let dh = display.handle();
        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "evilwm");
        seat.add_keyboard(Default::default(), 200, 25)
            .map_err(|error| io::Error::other(format!("failed to add keyboard seat: {error}")))?;
        seat.add_pointer();
        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;
        let loop_signal = event_loop.get_signal();

        let screen_size = Size::new(1280.0, 720.0);
        let mut viewport = Viewport::new(screen_size);
        let mut fallback_placement_policy = PlacementPolicy::default();
        if let Some(cfg) = &config {
            viewport = viewport.with_zoom_limits(cfg.canvas.min_zoom, cfg.canvas.max_zoom);
            fallback_placement_policy.default_size = Size::new(900.0, 600.0);
        }

        let bindings = config
            .as_ref()
            .map(|cfg| {
                BindingMap::from_config(&cfg.bindings, cfg.canvas.pan_step, cfg.canvas.zoom_step)
            })
            .unwrap_or_default();
        let window_rules = compile_window_rules(config.as_ref());

        let output_state = OutputState::with_viewport("winit", Point::new(0.0, 0.0), viewport);

        #[cfg(feature = "lua")]
        let live_lua = config_path
            .as_deref()
            .and_then(|path| {
                path.parent()
                    .map(std::path::Path::to_path_buf)
                    .map(|base| (path, base))
            })
            .map(|(path, base)| {
                let hooks = LiveLuaHooks::new(base)?;
                hooks.load_script_file(path)?;
                Ok::<LiveLuaHooks, crate::lua::ConfigError>(hooks)
            })
            .transpose()
            .map_err(|error| io::Error::other(format!("failed to initialize live lua hooks: {error}")))?;

        Ok(Self {
            start_time,
            socket_name,
            display_handle: dh,
            space,
            loop_signal,
            compositor_state,
            xdg_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
            output_state,
            output_states: BTreeMap::new(),
            focus_stack: FocusStack::default(),
            fallback_placement_policy,
            bindings,
            window_rules,
            next_window_id: 1,
            window_models: BTreeMap::new(),
            surface_window_ids: HashMap::new(),
            window_surfaces: HashMap::new(),
            config,
            config_path,
            redraw_requested: true,
            trackpad_pinch_scale: None,
            #[cfg(feature = "udev")]
            tty_control: None,
            #[cfg(feature = "udev")]
            tty_no_scanout_warned: false,
            #[cfg(feature = "lua")]
            live_lua,
            active_interactive_op: None,
            last_pointer_button_pressed: None,
            warned_missing_move_update_hook: false,
            warned_missing_resize_update_hook: false,
        })
    }

    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub fn redraw_requested(&self) -> bool {
        self.redraw_requested
    }

    pub fn clear_redraw_request(&mut self) {
        self.redraw_requested = false;
    }

    fn init_wayland_listener(
        display: Display<EvilWm>,
        event_loop: &mut EventLoop<Self>,
    ) -> Result<OsString, Box<dyn Error>> {
        let listening_socket = ListeningSocketSource::new_auto()
            .map_err(|error| io::Error::other(format!("failed to create wayland socket: {error}")))?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                if let Err(error) = state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                {
                    eprintln!("failed to insert wayland client: {error}");
                }
            })
            .map_err(|error| io::Error::other(format!("failed to init wayland listener: {error}")))?;

        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // SAFETY: calloop dispatches this source on the compositor thread with
                    // exclusive mutable access to the display during the callback.
                    let result = unsafe { display.get_mut().dispatch_clients(state) };
                    if let Err(error) = result {
                        eprintln!("wayland client dispatch error: {error}");
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|error| io::Error::other(format!("failed to register wayland dispatch source: {error}")))?;

        Ok(socket_name)
    }

    pub fn surface_under(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<(WlSurface, SmithayPoint<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }

    fn primary_output_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let output = self.space.outputs().next()?;
        self.space.output_geometry(output)
    }

    fn clamp_pointer_to_primary_output_or_viewport(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> SmithayPoint<f64, Logical> {
        if let Some(output_geo) = self.primary_output_geometry() {
            let min_x = output_geo.loc.x as f64;
            let min_y = output_geo.loc.y as f64;
            let max_x = min_x + output_geo.size.w as f64;
            let max_y = min_y + output_geo.size.h as f64;
            return (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into();
        }

        let fallback = self.viewport().screen_size();
        (
            pos.x.clamp(0.0, fallback.w.max(1.0)),
            pos.y.clamp(0.0, fallback.h.max(1.0)),
        )
            .into()
    }

    fn absolute_pointer_position<B: InputBackend, I: AbsolutePositionEvent<B>>(
        &self,
        event: &I,
    ) -> SmithayPoint<f64, Logical> {
        if let Some(output_geo) = self.primary_output_geometry() {
            return event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        }

        let fallback = self.viewport().screen_size();
        SmithayPoint::from((
            event.x_transformed(fallback.w.max(1.0) as i32),
            event.y_transformed(fallback.h.max(1.0) as i32),
        ))
    }

    fn viewport_pointer_anchor(&self) -> Point {
        let local_pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .map(|pointer| {
                let logical = self.output_state.logical_position();
                Point::new(pointer.x - logical.x, pointer.y - logical.y)
            });

        let screen = self.viewport().screen_size();
        let fallback = Point::new(screen.w / 2.0, screen.h / 2.0);
        let pointer = local_pointer.unwrap_or(fallback);
        Point::new(pointer.x.clamp(0.0, screen.w), pointer.y.clamp(0.0, screen.h))
    }

    fn hovered_window_snapshot_at(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<WindowSnapshot> {
        self.surface_under(pos)
            .and_then(|(surface, _)| self.window_id_for_surface(&surface))
            .and_then(|id| self.window_snapshot_for_id(id))
    }

    fn handle_pointer_position(
        &mut self,
        pos: SmithayPoint<f64, Logical>,
        serial: Serial,
        time: u32,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let under = self.surface_under(pos);
        let pointer_grabbed = pointer.is_grabbed();
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial,
                time,
            },
        );
        pointer.frame(self);

        let current = Point::new(pos.x, pos.y);
        let previous_focus = self.focus_stack.focused();
        let hovered_window = if pointer_grabbed {
            None
        } else {
            self.hovered_window_snapshot_at(pos)
        };
        let pending_trigger = if let Some(active) = self.active_interactive_op.as_mut() {
            let delta = active.advance(current);
            if delta.x != 0.0 || delta.y != 0.0 {
                Some((active.kind, active.window_id, delta))
            } else {
                None
            }
        } else {
            None
        };
        if let Some((kind, id, delta)) = pending_trigger {
            let resize_edges = self
                .active_interactive_op
                .as_ref()
                .and_then(|active| active.resize_edges());
            match kind {
                ActiveInteractiveKind::Move => self.trigger_live_move_update(id, delta, current),
                ActiveInteractiveKind::Resize => self.trigger_live_resize_update(
                    id,
                    delta,
                    current,
                    resize_edges.unwrap_or(ResizeEdges::all()),
                ),
            }
            if !self.window_models.contains_key(&id) {
                self.active_interactive_op = None;
            }
        }

        if !pointer_grabbed {
            let _ = self.try_live_resolve_focus(
                "pointer_motion",
                hovered_window,
                previous_focus,
                Some(current),
                None,
                None,
            );
        }
    }

    fn apply_trackpad_swipe(&mut self, delta: SmithayPoint<f64, Logical>) {
        self.pan_all_viewports(crate::canvas::Vec2::new(-delta.x, -delta.y));
    }

    fn begin_trackpad_pinch(&mut self) {
        self.trackpad_pinch_scale = Some(1.0);
    }

    fn apply_trackpad_pinch(
        &mut self,
        delta: SmithayPoint<f64, Logical>,
        absolute_scale: f64,
    ) {
        self.apply_trackpad_swipe(delta);

        if let Some(relative) = pinch_relative_factor(&mut self.trackpad_pinch_scale, absolute_scale)
            && (relative - 1.0).abs() > f64::EPSILON
        {
            let anchor = self.viewport_pointer_anchor();
            self.zoom_all_viewports_at_primary(anchor, relative);
        }
    }

    fn end_trackpad_pinch(&mut self) {
        self.trackpad_pinch_scale = None;
    }

    #[cfg(feature = "lua")]
    fn live_hook_exists(&mut self, hook_name: &str) -> bool {
        self.with_live_lua(|hooks, _| hooks.has_hook(hook_name))
            .unwrap_or(Ok(false))
            .unwrap_or_else(|error| {
                eprintln!("{}", format_live_hook_error(hook_name, &error));
                false
            })
    }

    fn warn_missing_move_update_hook(&mut self) {
        if self.warned_missing_move_update_hook {
            return;
        }
        self.warned_missing_move_update_hook = true;
        eprintln!(
            "interactive move requested, but no Lua move_update hook is installed; drag will do nothing"
        );
    }

    fn warn_missing_resize_update_hook(&mut self) {
        if self.warned_missing_resize_update_hook {
            return;
        }
        self.warned_missing_resize_update_hook = true;
        eprintln!(
            "interactive resize requested, but no Lua resize_update hook is installed; resize will do nothing"
        );
    }

    fn handle_resolved_key(&mut self, key: &str, modifiers: ModifierSet) -> bool {
        let keyspec = format_keyspec(key, modifiers);
        let bound_action = self.bindings.resolve(key, modifiers);
        let intercepted = bound_action.is_some();
        let hook_handled = self.trigger_live_key(keyspec);

        if !hook_handled
            && let Some(action) = bound_action
        {
            self.handle_action(action);
        }

        intercepted
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                keyboard.input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |state, modifiers, handle| {
                        if key_state == KeyState::Pressed {
                            let key = handle
                                .raw_latin_sym_or_raw_current_sym()
                                .map(xkb::keysym_get_name)
                                .unwrap_or_else(|| xkb::keysym_get_name(handle.modified_sym()));
                            let key = crate::input::bindings::normalize_key(&key);
                            let modifier_set = modifier_set_from(modifiers);
                            #[cfg(feature = "udev")]
                            if let Some(control) = default_tty_control_action(&key, modifier_set)
                                && let Some(callback) = state.tty_control.as_ref()
                            {
                                callback.borrow_mut()(control);
                                return FilterResult::Intercept(());
                            }
                            let intercepted = state.handle_resolved_key(&key, modifier_set);
                            if intercepted {
                                return FilterResult::Intercept(());
                            }
                        }
                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let pos = self.clamp_pointer_to_primary_output_or_viewport(
                    pointer.current_location() + event.delta(),
                );
                self.handle_pointer_position(pos, serial, event.time_msec());
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let pos = self.absolute_pointer_position(&event);
                let serial = SERIAL_COUNTER.next_serial();
                self.handle_pointer_position(pos, serial, event.time_msec());
            }
            InputEvent::PointerButton { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };

                if ButtonState::Pressed == button_state {
                    self.last_pointer_button_pressed = Some(button);
                }

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    let previous_focus = self.focus_stack.focused();
                    let pointer_point = Point::new(
                        pointer.current_location().x,
                        pointer.current_location().y,
                    );
                    let hovered_window = self
                        .space
                        .element_under(pointer.current_location())
                        .and_then(|(window, _location)| self.window_snapshot_for_space_window(window));

                    let _ = self.try_live_resolve_focus(
                        "pointer_button",
                        hovered_window,
                        previous_focus,
                        Some(pointer_point),
                        Some(button),
                        Some(true),
                    );
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);

                if ButtonState::Released == button_state
                    && self.last_pointer_button_pressed == Some(button)
                {
                    self.last_pointer_button_pressed = None;
                }

                if ButtonState::Released == button_state
                    && let Some(active) = self.active_interactive_op
                    && active.should_end_on_button(button)
                {
                    let Some(active) = self.active_interactive_op.take() else {
                        return;
                    };
                    let pointer_pos = self
                        .seat
                        .get_pointer()
                        .map(|pointer| pointer.current_location())
                        .unwrap_or_else(|| (0.0, 0.0).into());
                    let pointer_pos = Point::new(pointer_pos.x, pointer_pos.y);
                    match active.kind {
                        ActiveInteractiveKind::Move => self.trigger_live_move_end(
                            active.window_id,
                            active.total_delta(),
                            pointer_pos,
                        ),
                        ActiveInteractiveKind::Resize => self.trigger_live_resize_end(
                            active.window_id,
                            active.total_delta(),
                            pointer_pos,
                            active.resize_edges().unwrap_or(ResizeEdges::all()),
                        ),
                    }
                }
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();
                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.0
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.0
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }
                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            InputEvent::GestureSwipeBegin { event, .. } => {
                self.trigger_live_gesture(
                    "swipe_begin",
                    event.fingers(),
                    crate::canvas::Vec2::new(0.0, 0.0),
                    None,
                );
            }
            InputEvent::GestureSwipeUpdate { event, .. } => {
                let delta = event.delta();
                self.apply_trackpad_swipe(delta);
                self.trigger_live_gesture(
                    "swipe_update",
                    0,
                    crate::canvas::Vec2::new(delta.x, delta.y),
                    None,
                );
            }
            InputEvent::GestureSwipeEnd { .. } => {
                self.trigger_live_gesture(
                    "swipe_end",
                    0,
                    crate::canvas::Vec2::new(0.0, 0.0),
                    None,
                );
            }
            InputEvent::GesturePinchBegin { event, .. } => {
                self.begin_trackpad_pinch();
                self.trigger_live_gesture(
                    "pinch_begin",
                    event.fingers(),
                    crate::canvas::Vec2::new(0.0, 0.0),
                    Some(1.0),
                );
            }
            InputEvent::GesturePinchUpdate { event, .. } => {
                let delta = event.delta();
                self.apply_trackpad_pinch(delta, event.scale());
                self.trigger_live_gesture(
                    "pinch_update",
                    0,
                    crate::canvas::Vec2::new(delta.x, delta.y),
                    Some(event.scale()),
                );
            }
            InputEvent::GesturePinchEnd { .. } => {
                self.end_trackpad_pinch();
                self.trigger_live_gesture(
                    "pinch_end",
                    0,
                    crate::canvas::Vec2::new(0.0, 0.0),
                    None,
                );
            }
            _ => {}
        }
        self.request_redraw();
    }

    fn primary_output_name(&self) -> Option<String> {
        self.space
            .outputs()
            .next()
            .map(|output| output.name())
            .or_else(|| self.output_states.keys().next().cloned())
    }

    fn output_state_for_name(&self, name: &str) -> Option<&OutputState> {
        self.output_states.get(name)
    }

    fn output_state_for_name_mut(&mut self, name: &str) -> Option<&mut OutputState> {
        self.output_states.get_mut(name)
    }

    fn output_state_for_output(&self, output: &Output) -> Option<&OutputState> {
        self.output_state_for_name(&output.name())
    }

    fn register_output_state(
        &mut self,
        name: impl Into<String>,
        logical_position: Point,
        screen_size: Size,
    ) {
        let name = name.into();
        let mut viewport = self
            .primary_output_name()
            .and_then(|primary| self.output_states.get(&primary).map(|state| state.viewport().clone()))
            .unwrap_or_else(|| self.output_state.viewport().clone());
        viewport.set_screen_size(screen_size);

        self.output_states.insert(
            name.clone(),
            OutputState::with_viewport(name, logical_position, viewport),
        );
    }

    fn sync_output_state(&mut self, name: &str, logical_position: Point, screen_size: Size) {
        if let Some(state) = self.output_state_for_name_mut(name) {
            state.set_logical_position(logical_position);
            state.viewport_mut().set_screen_size(screen_size);
        } else {
            self.register_output_state(name.to_string(), logical_position, screen_size);
        }
    }

    #[cfg(feature = "udev")]
    fn remove_output_state(&mut self, name: &str) {
        self.output_states.remove(name);
    }

    fn clone_primary_camera_to_all_outputs(&mut self) {
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

    fn pan_all_viewports(&mut self, delta: crate::canvas::Vec2) {
        if let Some(primary_name) = self.primary_output_name()
            && let Some(state) = self.output_state_for_name_mut(&primary_name)
        {
            state.viewport_mut().pan_world(delta);
            self.clone_primary_camera_to_all_outputs();
            return;
        }

        self.output_state.viewport_mut().pan_world(delta);
    }

    fn zoom_all_viewports_at_primary(&mut self, anchor: Point, factor: f64) {
        if let Some(primary_name) = self.primary_output_name()
            && let Some(state) = self.output_state_for_name_mut(&primary_name)
        {
            state.viewport_mut().zoom_at_screen(anchor, factor);
            self.clone_primary_camera_to_all_outputs();
            return;
        }

        self.output_state.viewport_mut().zoom_at_screen(anchor, factor);
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

    #[cfg(feature = "udev")]
    pub(crate) fn sync_output_positions_to_viewport(&mut self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();

        let mut offset_x = 0;
        for output in outputs {
            let location = (offset_x, 0);
            output.change_current_state(None, None, None, Some(location.into()));
            self.space.map_output(&output, location);
            if let Some(geometry) = self.space.output_geometry(&output) {
                self.sync_output_state(
                    &output.name(),
                    Point::new(geometry.loc.x as f64, geometry.loc.y as f64),
                    Size::new(geometry.size.w as f64, geometry.size.h as f64),
                );
                offset_x += geometry.size.w;
            }
        }
    }

    #[cfg(feature = "udev")]
    pub(crate) fn sync_primary_output_state_from_space(&mut self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        if outputs.is_empty() {
            self.output_state.set_name("no-output");
            self.output_state.set_logical_position(Point::new(0.0, 0.0));
            self.output_states.clear();
            return;
        }

        let live_names = outputs.iter().map(Output::name).collect::<std::collections::BTreeSet<_>>();
        self.output_states.retain(|name, _| live_names.contains(name));

        for output in &outputs {
            if let Some(geometry) = self.space.output_geometry(output) {
                self.sync_output_state(
                    &output.name(),
                    Point::new(geometry.loc.x as f64, geometry.loc.y as f64),
                    Size::new(geometry.size.w as f64, geometry.size.h as f64),
                );
            }
        }

        let primary = outputs[0].clone();
        if let Some(primary_state) = self.output_state_for_output(&primary).cloned() {
            self.output_state = primary_state;
        }
    }

    #[cfg(feature = "udev")]
    pub(crate) fn center_pointer_on_primary_output(&mut self) {
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(geometry) = self.space.output_geometry(&output) else {
            return;
        };
        let center = (
            geometry.loc.x as f64 + geometry.size.w as f64 / 2.0,
            geometry.loc.y as f64 + geometry.size.h as f64 / 2.0,
        )
            .into();
        if let Some(pointer) = self.seat.get_pointer() {
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

    pub fn focus_window(&mut self, id: WindowId) -> bool {
        if !self.window_models.contains_key(&id) {
            return false;
        }

        let previous = self.focus_stack.focused();
        self.focus_stack.focus(id);
        self.sync_focus_to_stack();
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
        self.trigger_live_focus_changed(previous, None);
        self.request_redraw();
        true
    }

    pub fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        if let Some(window) = self.window_models.get_mut(&id) {
            window.bounds.origin = Point::new(x, y);
            if let Some(space_window) = self.find_space_window(id) {
                self.space
                    .map_element(space_window, (x.round() as i32, y.round() as i32), true);
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
            if let Some(space_window) = self.find_space_window(id)
                && let Some(toplevel) = window_toplevel(&space_window)
            {
                toplevel.with_pending_state(|state| {
                    state.size = Some((w.round() as i32, h.round() as i32).into());
                });
                toplevel.send_configure();
            }
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

        if let Some(space_window) = self.find_space_window(id)
            && let Some(toplevel) = window_toplevel(&space_window)
        {
            toplevel.send_close();
            return true;
        }

        false
    }

    fn sync_focus_to_stack(&mut self) {
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

    fn handle_action(&mut self, action: crate::input::Action) {
        match action {
            crate::input::Action::CloseWindow => self.close_focused_window(),
            crate::input::Action::Spawn { command } => spawn_client(&command, &self.socket_name),
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
            crate::input::Action::ZoomIn { factor }
            | crate::input::Action::ZoomOut { factor } => {
                let anchor = Point::new(
                    self.viewport().screen_size().w / 2.0,
                    self.viewport().screen_size().h / 2.0,
                );
                self.zoom_all_viewports_at_primary(anchor, factor);
            }
        }
        self.request_redraw();
    }

    fn close_focused_window(&mut self) {
        if let Some(id) = self.focus_stack.focused() {
            let _ = self.close_window(id);
        }
    }

    fn placement_rect_for(&self, properties: &WindowProperties) -> Rect {
        let existing = self.window_models.values().cloned().collect::<Vec<_>>();
        let requested_size = AppliedWindowRules::from_rules(properties, &self.window_rules).default_size;
        self.fallback_placement_policy
            .place_new_window(self.viewport(), &existing, requested_size)
            .bounds
    }

    fn apply_rules_to_window(window: &mut WindowModel, applied_rules: &AppliedWindowRules) {
        if let Some(floating) = applied_rules.floating {
            window.floating = floating;
        }
        if let Some(exclude_from_focus) = applied_rules.exclude_from_focus {
            window.exclude_from_focus = exclude_from_focus;
        }
    }

    fn sync_window_from_toplevel(
        &mut self,
        surface: &ToplevelSurface,
        send_configure_for_initial_size: bool,
    ) -> Option<WindowId> {
        let id = self.window_id_for_surface(surface.wl_surface())?;
        let properties = window_properties_from_toplevel(surface);

        let (default_size, size_changed) = {
            let window = self.window_models.get_mut(&id)?;
            window.properties = properties;

            let applied_rules = AppliedWindowRules::from_rules(&window.properties, &self.window_rules);
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

            (default_size, size_changed)
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

        self.request_redraw();
        Some(id)
    }

    fn track_new_window(
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

        self.window_models.insert(id, window);
        self.surface_window_ids.insert(surface.clone(), id);
        self.window_surfaces.insert(id, surface);
        id
    }

    fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.surface_window_ids.get(surface).copied()
    }

    fn window_snapshot_for_id(&self, id: WindowId) -> Option<WindowSnapshot> {
        let window = self.window_models.get(&id)?;
        Some(WindowSnapshot {
            id: window.id.0,
            app_id: window.properties.app_id.clone(),
            title: window.properties.title.clone(),
            bounds: window.bounds,
            floating: window.floating,
            exclude_from_focus: window.exclude_from_focus,
            focused: self.focus_stack.focused() == Some(window.id),
        })
    }

    fn window_snapshot_for_space_window(&self, window: &Window) -> Option<WindowSnapshot> {
        let surface = window_toplevel(window)?.wl_surface().clone();
        let id = self.window_id_for_surface(&surface)?;
        self.window_snapshot_for_id(id)
    }

    fn try_live_resolve_focus(
        &mut self,
        reason: &str,
        window: Option<WindowSnapshot>,
        previous: Option<WindowId>,
        pointer: Option<Point>,
        button: Option<u32>,
        pressed: Option<bool>,
    ) -> bool {
        #[cfg(feature = "lua")]
        if let Some(result) = self.with_live_lua(|hooks, state| {
            hooks.trigger_resolve_focus(
                state,
                ResolveFocusRequest {
                    reason,
                    window: window.as_ref(),
                    previous,
                    pointer,
                    button,
                    pressed,
                },
            )
        }) {
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

    fn untrack_surface(&mut self, surface: &WlSurface) {
        let removed = self
            .window_id_for_surface(surface)
            .and_then(|id| self.window_models.get(&id).cloned())
            .into_iter()
            .collect::<Vec<_>>();
        let previous_focus = self.focus_stack.focused();

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
                WindowSnapshot {
                    id: window.id.0,
                    app_id: window.properties.app_id.clone(),
                    title: window.properties.title.clone(),
                    bounds: window.bounds,
                    floating: window.floating,
                    exclude_from_focus: window.exclude_from_focus,
                    focused,
                }
            })
            .collect::<Vec<_>>();

        let snapshot = self.state_snapshot();
        for window_snapshot in &removed_snapshots {
            self.trigger_live_window_unmapped(snapshot.clone(), window_snapshot.clone());
        }

        let _ = removed_snapshots.last().cloned().map(|window_snapshot| {
            self.try_live_resolve_focus(
                "window_unmapped",
                Some(window_snapshot),
                previous_focus,
                None,
                None,
                None,
            )
        });

        self.request_redraw();
        self.sync_focus_to_stack();
    }

    fn cleanup_window_bookkeeping(&mut self) {
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

    fn find_space_window(&self, id: WindowId) -> Option<Window> {
        let surface = self.window_surfaces.get(&id)?;
        self.space
            .elements()
            .find(|window| window_matches_surface(window, surface))
            .cloned()
    }

    #[cfg(feature = "lua")]
    fn with_live_lua<R, F>(&mut self, f: F) -> Option<R>
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> R,
    {
        let hooks = self.live_lua.take()?;
        let result = f(&hooks, self);
        self.live_lua = Some(hooks);
        Some(result)
    }

    #[cfg(feature = "lua")]
    fn run_live_hook<F>(&mut self, hook_name: &str, f: F)
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> Result<bool, crate::lua::ConfigError>,
    {
        if let Some(result) = self.with_live_lua(f) {
            report_live_hook_result(hook_name, result);
        }
    }

    #[cfg(feature = "lua")]
    fn run_live_hook_result<F>(&mut self, hook_name: &str, f: F) -> bool
    where
        F: FnOnce(&LiveLuaHooks, &mut Self) -> Result<bool, crate::lua::ConfigError>,
    {
        self.with_live_lua(f)
            .map(|result| match result {
                Ok(handled) => handled,
                Err(error) => {
                    eprintln!("{}", format_live_hook_error(hook_name, &error));
                    false
                }
            })
            .unwrap_or(false)
    }

    fn trigger_live_place_window(&mut self, id: WindowId) {
        #[cfg(feature = "lua")]
        self.run_live_hook("place_window", |hooks, state| hooks.trigger_place_window(state, id));
    }

    fn trigger_live_window_mapped(&mut self, id: WindowId) {
        #[cfg(feature = "lua")]
        self.run_live_hook("window_mapped", |hooks, state| hooks.trigger_window_mapped(state, id));
    }

    fn trigger_live_window_unmapped(
        &mut self,
        snapshot: RuntimeStateSnapshot,
        window: WindowSnapshot,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("window_unmapped", |hooks, state| {
            hooks.trigger_window_unmapped(state, &snapshot, &window)
        });
    }

    fn trigger_live_focus_changed(
        &mut self,
        previous: Option<WindowId>,
        current: Option<WindowId>,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("focus_changed", |hooks, state| {
            hooks.trigger_focus_changed(state, previous, current)
        });
    }

    fn trigger_live_key(&mut self, keyspec: String) -> bool {
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

    fn trigger_live_gesture(
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

    fn trigger_live_move_begin(&mut self, id: WindowId) {
        #[cfg(feature = "lua")]
        if !self.live_hook_exists("move_update") {
            self.warn_missing_move_update_hook();
        }

        #[cfg(feature = "lua")]
        self.run_live_hook("move_begin", |hooks, state| hooks.trigger_move_begin(state, id));
    }

    fn trigger_live_move_update(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("move_update", |hooks, state| {
            hooks.trigger_move_update(state, id, delta, Some(pointer))
        });
    }

    fn trigger_live_move_end(&mut self, id: WindowId, delta: crate::canvas::Vec2, pointer: Point) {
        #[cfg(feature = "lua")]
        self.run_live_hook("move_end", |hooks, state| {
            hooks.trigger_move_end(state, id, delta, Some(pointer))
        });
    }

    fn trigger_live_resize_begin(&mut self, id: WindowId, edges: ResizeEdges) {
        #[cfg(feature = "lua")]
        if !self.live_hook_exists("resize_update") {
            self.warn_missing_resize_update_hook();
        }

        #[cfg(feature = "lua")]
        self.run_live_hook("resize_begin", |hooks, state| {
            hooks.trigger_resize_begin(state, id, edges)
        });
    }

    fn trigger_live_resize_update(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
        edges: ResizeEdges,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("resize_update", |hooks, state| {
            hooks.trigger_resize_update(state, id, delta, Some(pointer), edges)
        });
    }

    fn trigger_live_resize_end(
        &mut self,
        id: WindowId,
        delta: crate::canvas::Vec2,
        pointer: Point,
        edges: ResizeEdges,
    ) {
        #[cfg(feature = "lua")]
        self.run_live_hook("resize_end", |hooks, state| {
            hooks.trigger_resize_end(state, id, delta, Some(pointer), edges)
        });
    }
}

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
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(window) = self
                .space
                .elements()
                .find(|window| window_matches_surface(window, &root))
            {
                window.on_commit();
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

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let properties = window_properties_from_toplevel(&surface);
        let fallback_rect = self.placement_rect_for(&properties);
        let previous_focus = self.focus_stack.focused();
        let id = self.track_new_window(surface.wl_surface().clone(), fallback_rect, properties);
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

        let _ = self.try_live_resolve_focus(
            "window_mapped",
            self.window_snapshot_for_id(id),
            previous_focus,
            None,
            None,
            None,
        );

        let configured_rect = self
            .window_models
            .get(&id)
            .map(|window| window.bounds)
            .unwrap_or(configured_rect);
        surface.with_pending_state(|state| {
            if self.focus_stack.focused() == Some(id) {
                state.states.set(xdg_toplevel::State::Activated);
            }
            state.size = Some(
                (
                    configured_rect.size.w.round() as i32,
                    configured_rect.size.h.round() as i32,
                )
                    .into(),
            );
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

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        let Some(id) = self.window_id_for_surface(surface.wl_surface()) else {
            return;
        };
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
    }

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
        let pointer = self
            .seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_else(|| (0.0, 0.0).into());
        let resize_edges = resize_edges_from(edges);
        self.active_interactive_op = Some(ActiveInteractiveOp::new(
            ActiveInteractiveKind::Resize,
            id,
            Point::new(pointer.x, pointer.y),
            Some(resize_edges),
            self.last_pointer_button_pressed,
        ));
        self.trigger_live_resize_begin(id, resize_edges);
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        let _ = self.sync_window_from_toplevel(&surface, true);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let _ = self.sync_window_from_toplevel(&surface, true);
    }
}

fn window_toplevel(window: &Window) -> Option<ToplevelSurface> {
    window.toplevel().cloned()
}

fn window_matches_surface(window: &Window, surface: &WlSurface) -> bool {
    window_toplevel(window).is_some_and(|toplevel| toplevel.wl_surface() == surface)
}

impl EvilWm {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|window| window_matches_surface(window, &root))
        else {
            return;
        };
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geo) = self.space.element_geometry(window) else {
            return;
        };

        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
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

        if !initial_configure_sent
            && let Some(toplevel) = window_toplevel(&window)
        {
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
        set_data_device_focus(&self.display_handle, seat, client);
    }
}

impl SelectionHandler for EvilWm {
    type SelectionUserData = ();
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

impl OutputHandler for EvilWm {}

#[cfg(test)]
fn apply_trackpad_swipe_to_viewport(
    viewport: &mut crate::canvas::Viewport,
    delta: SmithayPoint<f64, Logical>,
) {
    viewport.pan_world(crate::canvas::Vec2::new(-delta.x, -delta.y));
}

fn pinch_relative_factor(previous_scale: &mut Option<f64>, absolute_scale: f64) -> Option<f64> {
    let clamped_scale = absolute_scale.max(0.0001);
    let previous = previous_scale.replace(clamped_scale)?;
    Some(clamped_scale / previous.max(0.0001))
}

fn modifier_set_from(modifiers: &ModifiersState) -> ModifierSet {
    ModifierSet {
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
        logo: modifiers.logo,
    }
}

fn resize_edges_from(edge: xdg_toplevel::ResizeEdge) -> ResizeEdges {
    match edge {
        xdg_toplevel::ResizeEdge::Top => ResizeEdges {
            left: false,
            right: false,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::Bottom => ResizeEdges {
            left: false,
            right: false,
            top: false,
            bottom: true,
        },
        xdg_toplevel::ResizeEdge::Left => ResizeEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::TopLeft => ResizeEdges {
            left: true,
            right: false,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::BottomLeft => ResizeEdges {
            left: true,
            right: false,
            top: false,
            bottom: true,
        },
        xdg_toplevel::ResizeEdge::Right => ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::TopRight => ResizeEdges {
            left: false,
            right: true,
            top: true,
            bottom: false,
        },
        xdg_toplevel::ResizeEdge::BottomRight => ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        },
        _ => ResizeEdges::all(),
    }
}

impl ActionTarget for EvilWm {
    fn move_window(&mut self, id: WindowId, x: f64, y: f64) -> bool {
        Self::move_window(self, id, x, y)
    }

    fn resize_window(&mut self, id: WindowId, w: f64, h: f64) -> bool {
        Self::resize_window(self, id, w, h)
    }

    fn set_window_bounds(&mut self, id: WindowId, bounds: Rect) -> bool {
        Self::set_window_bounds(self, id, bounds)
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
        self.pan_all_viewports(crate::canvas::Vec2::new(dx, dy));
    }

    fn zoom_canvas(&mut self, factor: f64) -> Result<(), ConfigError> {
        if factor <= 0.0 {
            return Err(ConfigError::Validation(
                "hook action zoom_canvas requires factor > 0".into(),
            ));
        }
        let screen = self.viewport().screen_size();
        self.zoom_all_viewports_at_primary(Point::new(screen.w / 2.0, screen.h / 2.0), factor);
        Ok(())
    }
}

fn format_keyspec(key: &str, modifiers: ModifierSet) -> String {
    let mut parts = Vec::new();
    if modifiers.logo {
        parts.push("Super".to_string());
    }
    if modifiers.ctrl {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    parts.push(key.to_string());
    parts.join("+")
}

#[cfg(feature = "lua")]
fn format_live_hook_error(hook_name: &str, error: &crate::lua::ConfigError) -> String {
    format!("live lua hook {hook_name} failed: {error}")
}

#[cfg(feature = "lua")]
fn report_live_hook_result(hook_name: &str, result: Result<bool, crate::lua::ConfigError>) {
    if let Err(error) = result {
        eprintln!("{}", format_live_hook_error(hook_name, &error));
    }
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn output_snapshot_for_render(state: &EvilWm, output: &Output) -> Option<OutputSnapshot> {
    let output_state = state.output_state_for_output(output)?;
    let viewport = output_state.viewport();
    let logical_position = output_state.logical_position();
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
}

#[cfg(feature = "lua")]
fn draw_commands_for_output(state: &mut EvilWm, output: &Output, hook_name: &str) -> Vec<DrawCommand> {
    let Some(output_snapshot) = output_snapshot_for_render(state, output) else {
        return Vec::new();
    };
    if let Some(result) = state.with_live_lua(|hooks, state| {
        hooks.draw_commands_for_output(state, hook_name, &output_snapshot)
    }) {
        match result {
            Ok(commands) => commands,
            Err(error) => {
                eprintln!("{}", format_live_hook_error(hook_name, &error));
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

#[cfg(not(feature = "lua"))]
fn draw_commands_for_output(_state: &mut EvilWm, _output: &Output, _hook_name: &str) -> Vec<DrawCommand> {
    Vec::new()
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn draw_rect_to_physical(
    state: &EvilWm,
    output: &Output,
    space: DrawSpace,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Option<Rectangle<i32, Physical>> {
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let viewport = state.output_state_for_output(output)?.viewport();
    let (screen_x, screen_y, screen_w, screen_h) = match space {
        DrawSpace::Screen => (x, y, w, h),
        DrawSpace::World => {
            let top_left = viewport.world_to_screen(Point::new(x, y));
            let bottom_right = viewport.world_to_screen(Point::new(x + w, y + h));
            (
                top_left.x,
                top_left.y,
                bottom_right.x - top_left.x,
                bottom_right.y - top_left.y,
            )
        }
    };

    let width = screen_w.round().max(1.0) as i32;
    let height = screen_h.round().max(1.0) as i32;
    let left = screen_x.round() as i32;
    let top = screen_y.round() as i32;

    Some(Rectangle::<i32, Physical>::new(
        (left, top).into(),
        (width, height).into(),
    ))
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
fn stroke_rects(
    rect: Rectangle<i32, Physical>,
    width: i32,
    outer: i32,
) -> Vec<Rectangle<i32, Physical>> {
    if rect.size.w <= 0 || rect.size.h <= 0 || width <= 0 || outer < 0 {
        return Vec::new();
    }
    let x = rect.loc.x - outer;
    let y = rect.loc.y - outer;
    let w = rect.size.w + outer * 2;
    let h = rect.size.h + outer * 2;
    vec![
        Rectangle::<i32, Physical>::new((x, y).into(), (w, width).into()),
        Rectangle::<i32, Physical>::new((x, y + h - width).into(), (w, width).into()),
        Rectangle::<i32, Physical>::new((x, y).into(), (width, h).into()),
        Rectangle::<i32, Physical>::new((x + w - width, y).into(), (width, h).into()),
    ]
}

#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
pub(crate) fn solid_elements_from_draw_commands(
    state: &mut EvilWm,
    output: &Output,
    hook_name: &str,
) -> Vec<SolidColorRenderElement> {
    let commands = draw_commands_for_output(state, output, hook_name);
    let mut elements = Vec::new();
    for command in commands {
        match command {
            DrawCommand::Rect { space, x, y, w, h, color } => {
                if let Some(rect) = draw_rect_to_physical(state, output, space, x, y, w, h) {
                    elements.push(SolidColorRenderElement::new(
                        Id::new(),
                        rect,
                        0usize,
                        color,
                        Kind::Unspecified,
                    ));
                }
            }
            DrawCommand::StrokeRect { space, x, y, w, h, width, outer, color } => {
                if let Some(rect) = draw_rect_to_physical(state, output, space, x, y, w, h) {
                    for stroke in stroke_rects(rect, width.round().max(1.0) as i32, outer.round().max(0.0) as i32) {
                        elements.push(SolidColorRenderElement::new(
                            Id::new(),
                            stroke,
                            0usize,
                            color,
                            Kind::Unspecified,
                        ));
                    }
                }
            }
        }
    }
    elements
}

#[cfg(feature = "udev")]
fn output_visible_world_geometry(
    state: &EvilWm,
    output: &Output,
) -> Option<Rectangle<i32, Logical>> {
    let viewport = state.output_state_for_output(output)?.viewport();
    let top_left = viewport.screen_to_world(Point::new(0.0, 0.0));
    let bottom_right = viewport.screen_to_world(Point::new(
        viewport.screen_size().w,
        viewport.screen_size().h,
    ));

    let left = top_left.x.floor() as i32;
    let top = top_left.y.floor() as i32;
    let right = bottom_right.x.ceil() as i32;
    let bottom = bottom_right.y.ceil() as i32;

    Some(Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    ))
}

#[cfg(feature = "udev")]
pub(crate) fn build_live_space_elements(
    state: &EvilWm,
    renderer: &mut GlesRenderer,
    output: &Output,
) -> Vec<
    smithay::desktop::space::SpaceRenderElements<
        GlesRenderer,
        RescaleRenderElement<<Window as AsRenderElements<GlesRenderer>>::RenderElement>,
    >,
> {
    let Some(region) = output_visible_world_geometry(state, output) else {
        return Vec::new();
    };

    let output_scale = Scale::from(output.current_scale().fractional_scale());
    let Some(output_state) = state.output_state_for_output(output) else {
        return Vec::new();
    };
    let viewport_scale = Scale::from(output_state.viewport().zoom());
    state
        .space
        .render_elements_for_region(renderer, &region, output_scale, 1.0)
        .into_iter()
        .map(|element| RescaleRenderElement::from_element(element, (0, 0).into(), viewport_scale))
        .map(|element| smithay::desktop::space::SpaceRenderElements::Element(Wrap::from(element)))
        .collect()
}

#[cfg(feature = "udev")]
pub(crate) fn build_cursor_elements(
    state: &EvilWm,
    output: &Output,
) -> Vec<SolidColorRenderElement> {
    let Some(pointer) = state.seat.get_pointer() else {
        return Vec::new();
    };
    let Some(output_geo) = state.space.output_geometry(output) else {
        return Vec::new();
    };
    let pos = pointer.current_location();
    if pos.x < output_geo.loc.x as f64
        || pos.y < output_geo.loc.y as f64
        || pos.x >= (output_geo.loc.x + output_geo.size.w) as f64
        || pos.y >= (output_geo.loc.y + output_geo.size.h) as f64
    {
        return Vec::new();
    }

    let local_x = pos.x.round() as i32 - output_geo.loc.x;
    let local_y = pos.y.round() as i32 - output_geo.loc.y;

    vec![
        SolidColorRenderElement::new(
            Id::new(),
            Rectangle::<i32, Physical>::new((local_x, local_y).into(), (10, 10).into()),
            0usize,
            [0.95, 0.95, 0.98, 0.95],
            Kind::Cursor,
        ),
        SolidColorRenderElement::new(
            Id::new(),
            Rectangle::<i32, Physical>::new((local_x + 2, local_y + 2).into(), (2, 14).into()),
            0usize,
            [0.15, 0.12, 0.2, 1.0],
            Kind::Cursor,
        ),
    ]
}

delegate_xdg_shell!(EvilWm);
delegate_compositor!(EvilWm);
delegate_shm!(EvilWm);
delegate_seat!(EvilWm);
delegate_data_device!(EvilWm);
delegate_output!(EvilWm);

pub fn run_winit(options: RuntimeOptions) -> Result<(), Box<dyn std::error::Error>> {
    let startup_commands = startup_commands(&options);
    let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new()?;
    let display: Display<EvilWm> = Display::new()?;
    let mut state = EvilWm::new(
        &mut event_loop,
        display,
        options.config_path.clone(),
        options.config,
    )?;

    let (mut backend, winit) = winit::init()?;

    let mode = OutputMode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "evilwm".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "evilwm".into(),
            model: "winit".into(),
        },
    );
    let _global = output.create_global::<EvilWm>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));
    state.register_output_state(
        output.name(),
        Point::new(0.0, 0.0),
        Size::new(mode.size.w as f64, mode.size.h as f64),
    );
    state.output_state = state
        .output_state_for_output(&output)
        .cloned()
        .unwrap_or_else(|| OutputState::new(output.name(), Point::new(0.0, 0.0), Size::new(mode.size.w as f64, mode.size.h as f64)));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(OutputMode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                state.sync_output_state(
                    &output.name(),
                    Point::new(0.0, 0.0),
                    Size::new(size.w as f64, size.h as f64),
                );
                if let Some(output_state) = state.output_state_for_output(&output).cloned() {
                    state.output_state = output_state;
                }
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::Redraw => {
                let size = backend.window_size();
                let damage = Rectangle::from_size(size);
                {
                    let Ok((renderer, mut framebuffer)) = backend.bind() else {
                        eprintln!("winit backend bind failed during redraw");
                        state.request_redraw();
                        return;
                    };
                    let background_elements = solid_elements_from_draw_commands(
                        state,
                        &output,
                        "draw_background",
                    );
                    let overlay_elements = solid_elements_from_draw_commands(
                        state,
                        &output,
                        "draw_overlay",
                    );
                    let space_elements = match smithay::desktop::space::space_render_elements(
                        renderer,
                        [&state.space],
                        &output,
                        1.0,
                    ) {
                        Ok(elements) => elements,
                        Err(error) => {
                            eprintln!("winit render element generation failed: {error}");
                            state.request_redraw();
                            return;
                        }
                    };

                    let mut elements = Vec::<LiveRenderElements<GlesRenderer, _>>::with_capacity(
                        background_elements.len() + space_elements.len() + overlay_elements.len(),
                    );
                    elements.extend(background_elements.into_iter().map(LiveRenderElements::Custom));
                    elements.extend(space_elements.into_iter().map(LiveRenderElements::Space));
                    elements.extend(overlay_elements.into_iter().map(LiveRenderElements::Custom));

                    if let Err(error) = damage_tracker.render_output(
                        renderer,
                        &mut framebuffer,
                        0,
                        &elements,
                        [0.08, 0.05, 0.12, 1.0],
                    ) {
                        eprintln!("winit damage render failed: {error}");
                        state.request_redraw();
                        return;
                    }
                }
                if let Err(error) = backend.submit(Some(&[damage])) {
                    eprintln!("winit backend submit failed: {error}");
                    state.request_redraw();
                    return;
                }

                state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    )
                });
                state.space.refresh();
                state.cleanup_window_bookkeeping();
                state.popups.cleanup();
                let _ = state.display_handle.flush_clients();
                backend.window().request_redraw();
            }
            WinitEvent::CloseRequested => state.loop_signal.stop(),
            _ => {}
        })?;

    println!(
        "evilwm nested compositor running on WAYLAND_DISPLAY={}",
        state.socket_name.to_string_lossy()
    );
    if let Some(path) = options.config_path.as_deref() {
        println!("loaded config: {}", path.display());
    }
    publish_wayland_display(&state.socket_name);
    for command in startup_commands {
        println!("spawning client: {command}");
        spawn_client(&command, &state.socket_name);
    }

    event_loop.run(None, &mut state, move |_| {})?;
    Ok(())
}

pub(super) fn startup_commands(options: &RuntimeOptions) -> Vec<String> {
    let mut commands = options
        .config
        .as_ref()
        .map(|config| config.autostart.clone())
        .unwrap_or_default();

    if let Some(command) = options.command.as_ref() {
        commands.push(command.clone());
    }

    commands
}

pub(super) fn publish_wayland_display(socket_name: &std::ffi::OsStr) {
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
    }
}

pub(super) fn spawn_client(command: &str, wayland_display: &std::ffi::OsStr) {
    let Some(mut cmd) = build_spawn_command(command, wayland_display) else {
        return;
    };
    let _ = cmd.spawn();
}

fn build_spawn_command(
    command: &str,
    wayland_display: &std::ffi::OsStr,
) -> Option<std::process::Command> {
    if command.trim().is_empty() {
        return None;
    }

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c");
    cmd.arg(command);
    cmd.env("WAYLAND_DISPLAY", wayland_display);
    Some(cmd)
}

#[cfg(test)]
mod tests {
    use super::{
        ActiveInteractiveKind, ActiveInteractiveOp, apply_trackpad_swipe_to_viewport,
        build_spawn_command, compile_window_rules, pinch_relative_factor, resize_edges_from,
    };
    #[cfg(feature = "udev")]
    use super::{TtyControlAction, default_tty_control_action};
    #[cfg(feature = "lua")]
    use super::{format_live_hook_error, startup_commands};
    use crate::canvas::{Size, Viewport};
    #[cfg(any(feature = "lua", feature = "udev"))]
    use crate::canvas::Point as CanvasPoint;
    #[cfg(feature = "lua")]
    use crate::compositor::EvilWm;
    #[cfg(feature = "lua")]
    use crate::input::{BindingMap, ModifierSet};
    #[cfg(feature = "lua")]
    use crate::lua::{BindingConfig, CanvasConfig, Config, ConfigError, LiveLuaHooks};
    #[cfg(feature = "lua")]
    use crate::window::{Window, WindowRule};
    use crate::{canvas::Point, window::WindowId};
    use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
    #[cfg(any(feature = "lua", feature = "udev"))]
    use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
    use smithay::utils::Transform;
    use std::ffi::OsStr;

    #[cfg(feature = "lua")]
    fn create_live_test_state(config: Option<Config>) -> EvilWm {
        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        EvilWm::new(&mut event_loop, display, None, config).expect("state")
    }

    #[cfg(feature = "lua")]
    fn create_test_output(name: &str, loc: (i32, i32), size: (i32, i32)) -> Output {
        let output = Output::new(
            name.to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "evilwm-test".into(),
                model: "test".into(),
            },
        );
        let mode = OutputMode {
            size: size.into(),
            refresh: 60_000,
        };
        output.change_current_state(Some(mode), Some(Transform::Normal), None, Some(loc.into()));
        output.set_preferred(mode);
        output
    }

    #[cfg(feature = "lua")]
    #[test]
    fn multi_output_state_snapshot_enumerates_real_outputs() {
        let mut state = create_live_test_state(None);
        let left = create_test_output("left", (0, 0), (1280, 720));
        let right = create_test_output("right", (1280, 0), (1920, 1080));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (1280, 0));
        state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(1280.0, 720.0));
        state.register_output_state("right", CanvasPoint::new(1280.0, 0.0), Size::new(1920.0, 1080.0));

        let snapshot = state.state_snapshot();
        assert_eq!(snapshot.outputs.len(), 2);
        assert_eq!(snapshot.outputs[0].id, "left");
        assert_eq!(snapshot.outputs[1].id, "right");
        assert_eq!(snapshot.outputs[0].logical_x, 0.0);
        assert_eq!(snapshot.outputs[1].logical_x, 1280.0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn multi_output_visible_world_uses_per_output_viewports() {
        let mut state = create_live_test_state(None);
        let left = create_test_output("left", (0, 0), (800, 600));
        let right = create_test_output("right", (800, 0), (1600, 900));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (800, 0));
        state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        state.register_output_state("right", CanvasPoint::new(800.0, 0.0), Size::new(1600.0, 900.0));

        {
            let left_state = state.output_state_for_name_mut("left").expect("left output state");
            left_state.viewport_mut().pan_world(crate::canvas::Vec2::new(100.0, 50.0));
        }
        {
            let right_state = state.output_state_for_name_mut("right").expect("right output state");
            right_state.viewport_mut().pan_world(crate::canvas::Vec2::new(-20.0, 10.0));
            right_state.viewport_mut().zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);
        }

        let snapshot = state.state_snapshot();
        let left_output = snapshot.outputs.iter().find(|output| output.id == "left").expect("left snapshot");
        let right_output = snapshot.outputs.iter().find(|output| output.id == "right").expect("right snapshot");

        assert_eq!(left_output.viewport.visible_world, crate::canvas::Rect::new(100.0, 50.0, 800.0, 600.0));
        assert_eq!(right_output.viewport.visible_world, crate::canvas::Rect::new(-20.0, 10.0, 800.0, 450.0));
        assert_ne!(left_output.viewport.visible_world, right_output.viewport.visible_world);
    }

    #[test]
    fn interactive_op_reports_incremental_and_total_delta() {
        let mut op = ActiveInteractiveOp::new(
            ActiveInteractiveKind::Move,
            WindowId(7),
            Point::new(100.0, 200.0),
            None,
            Some(272),
        );

        let first = op.advance(Point::new(130.0, 250.0));
        assert_eq!(first.x, 30.0);
        assert_eq!(first.y, 50.0);

        let second = op.advance(Point::new(150.0, 255.0));
        assert_eq!(second.x, 20.0);
        assert_eq!(second.y, 5.0);

        let total = op.total_delta();
        assert_eq!(total.x, 50.0);
        assert_eq!(total.y, 55.0);
    }

    #[test]
    fn interactive_op_only_ends_on_initiating_button() {
        let op = ActiveInteractiveOp::new(
            ActiveInteractiveKind::Resize,
            WindowId(9),
            Point::new(0.0, 0.0),
            Some(crate::window::ResizeEdges::all()),
            Some(274),
        );
        assert!(!op.should_end_on_button(272));
        assert!(op.should_end_on_button(274));

        let op_without_button = ActiveInteractiveOp::new(
            ActiveInteractiveKind::Move,
            WindowId(10),
            Point::new(0.0, 0.0),
            None,
            None,
        );
        assert!(op_without_button.should_end_on_button(272));
    }

    #[test]
    fn trackpad_swipe_pans_viewport_like_canvas_drag() {
        let mut viewport = Viewport::new(Size::new(800.0, 600.0));
        apply_trackpad_swipe_to_viewport(&mut viewport, (30.0, -20.0).into());
        assert_eq!(viewport.world_origin().x, -30.0);
        assert_eq!(viewport.world_origin().y, 20.0);
    }

    #[test]
    fn pinch_relative_factor_uses_incremental_scale() {
        let mut previous = Some(1.0);
        let first = pinch_relative_factor(&mut previous, 1.10).expect("first relative factor");
        let second = pinch_relative_factor(&mut previous, 1.21).expect("second relative factor");

        assert!((first - 1.10).abs() < 1e-9);
        assert!((second - 1.10).abs() < 1e-9);
        assert_eq!(previous, Some(1.21));
    }

    #[test]
    fn build_spawn_command_sets_nested_wayland_display() {
        let cmd =
            build_spawn_command("foot --server", OsStr::new("wayland-99")).expect("spawn command");
        let envs = cmd.get_envs().collect::<Vec<_>>();
        assert!(envs.iter().any(|(key, value)| {
            key == &OsStr::new("WAYLAND_DISPLAY") && value == &Some(OsStr::new("wayland-99"))
        }));
        assert_eq!(cmd.get_program(), OsStr::new("sh"));
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("-c"), OsStr::new("foot --server")]
        );
    }

    #[test]
    fn build_spawn_command_preserves_shell_quoted_arguments() {
        let cmd = build_spawn_command(
            "foot --title \"hello world\" --server",
            OsStr::new("wayland-99"),
        )
        .expect("spawn command");
        assert_eq!(cmd.get_program(), OsStr::new("sh"));
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("-c"),
                OsStr::new("foot --title \"hello world\" --server"),
            ]
        );
    }

    #[test]
    fn build_spawn_command_rejects_empty_command() {
        assert!(build_spawn_command("   ", OsStr::new("wayland-99")).is_none());
    }

    #[cfg(feature = "lua")]
    #[test]
    fn compile_window_rules_preserves_matchers_and_actions() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            autostart: Vec::new(),
            bindings: Vec::new(),
            rules: vec![crate::lua::RuleConfig {
                app_id: Some("foot".into()),
                title_contains: Some("scratch".into()),
                floating: Some(false),
                exclude_from_focus: Some(true),
                width: Some(640.0),
                height: Some(480.0),
            }],
            source_root: std::path::PathBuf::from("."),
        };

        assert_eq!(
            compile_window_rules(Some(&config)),
            vec![WindowRule {
                app_id: Some("foot".into()),
                title_contains: Some("scratch".into()),
                floating: Some(false),
                exclude_from_focus: Some(true),
                default_size: Some(Size::new(640.0, 480.0)),
            }]
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_key_hook_runs_before_rust_binding_fallback() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            autostart: Vec::new(),
            bindings: vec![BindingConfig {
                mods: vec!["Super".into()],
                key: "H".into(),
                action: "pan_left".into(),
                amount: Some(32.0),
                command: None,
            }],
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        };
        let mut state = create_live_test_state(Some(config.clone()));
        state.bindings = BindingMap::from_config(&config.bindings, 64.0, 1.2);

        let id = WindowId(1);
        state
            .window_models
            .insert(id, Window::new(id, crate::canvas::Rect::new(100.0, 120.0, 300.0, 200.0)));
        state.focus_stack.focus(id);

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.key = function(ctx)
                  if ctx.bound_action == "pan_left" then
                    evil.window.move(1, 5, 7)
                  end
                end
                "#,
                "live-key.lua",
            )
            .expect("load hooks");
        state.live_lua = Some(hooks);

        assert!(state.handle_resolved_key(
            "H",
            ModifierSet {
                ctrl: false,
                alt: false,
                shift: false,
                logo: true,
            }
        ));

        let window = state.window_models.get(&id).expect("window exists");
        assert_eq!(window.bounds.origin, CanvasPoint::new(5.0, 7.0));
        assert_eq!(state.viewport().world_origin(), CanvasPoint::new(0.0, 0.0));
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_key_without_hook_falls_back_to_rust_binding() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            autostart: Vec::new(),
            bindings: vec![BindingConfig {
                mods: vec!["Super".into()],
                key: "H".into(),
                action: "pan_left".into(),
                amount: Some(32.0),
                command: None,
            }],
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        };
        let mut state = create_live_test_state(Some(config.clone()));
        state.bindings = BindingMap::from_config(&config.bindings, 64.0, 1.2);

        assert!(state.handle_resolved_key(
            "H",
            ModifierSet {
                ctrl: false,
                alt: false,
                shift: false,
                logo: true,
            }
        ));
        assert_eq!(state.viewport().world_origin(), CanvasPoint::new(-32.0, 0.0));
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_move_hook_sequence_updates_window_model() {
        let mut state = create_live_test_state(None);
        let id = WindowId(7);
        state
            .window_models
            .insert(id, Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)));

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.move_begin = function(ctx)
                  evil.window.move(ctx.window.id, 10, 20)
                end
                evil.on.move_update = function(ctx)
                  evil.window.move(ctx.window.id, 30, 40)
                end
                evil.on.move_end = function(ctx)
                  evil.window.move(ctx.window.id, 50, 60)
                end
                "#,
                "live-move.lua",
            )
            .expect("load move hooks");
        state.live_lua = Some(hooks);

        state.trigger_live_move_begin(id);
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(10.0, 20.0)
        );
        state.trigger_live_move_update(id, crate::canvas::Vec2::new(4.0, 6.0), CanvasPoint::new(4.0, 6.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(30.0, 40.0)
        );
        state.trigger_live_move_end(id, crate::canvas::Vec2::new(9.0, 12.0), CanvasPoint::new(9.0, 12.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(50.0, 60.0)
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_resize_hook_sequence_updates_window_model() {
        let mut state = create_live_test_state(None);
        let id = WindowId(9);
        state
            .window_models
            .insert(id, Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)));

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.resize_begin = function(ctx)
                  evil.window.resize(ctx.window.id, 320, 220)
                end
                evil.on.resize_update = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 5, 6, 360, 240)
                end
                evil.on.resize_end = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 7, 8, 400, 260)
                end
                "#,
                "live-resize.lua",
            )
            .expect("load resize hooks");
        state.live_lua = Some(hooks);

        let edges = crate::window::ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        state.trigger_live_resize_begin(id, edges);
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.size,
            Size::new(320.0, 220.0)
        );
        state.trigger_live_resize_update(id, crate::canvas::Vec2::new(6.0, 9.0), CanvasPoint::new(6.0, 9.0), edges);
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds,
            crate::canvas::Rect::new(5.0, 6.0, 360.0, 240.0)
        );
        state.trigger_live_resize_end(id, crate::canvas::Vec2::new(8.0, 10.0), CanvasPoint::new(8.0, 10.0), edges);
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds,
            crate::canvas::Rect::new(7.0, 8.0, 400.0, 260.0)
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn startup_commands_include_config_autostart_and_cli_command() {
        let options = super::RuntimeOptions {
            command: Some("wezterm".into()),
            config_path: None,
            config: Some(Config {
                backend: Some("winit".into()),
                canvas: CanvasConfig::default(),
                autostart: vec!["foot".into(), "waybar".into()],
                bindings: Vec::new(),
                rules: Vec::new(),
                source_root: std::path::PathBuf::from("."),
            }),
        };

        assert_eq!(
            startup_commands(&options),
            vec![
                "foot".to_string(),
                "waybar".to_string(),
                "wezterm".to_string(),
            ]
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn startup_commands_handle_missing_config_and_command() {
        let options = super::RuntimeOptions {
            command: None,
            config_path: None,
            config: None,
        };

        assert!(startup_commands(&options).is_empty());
    }

    #[cfg(feature = "udev")]
    #[test]
    fn default_tty_control_binds_quit_and_vt_switches() {
        let modifiers = ModifierSet {
            ctrl: true,
            alt: true,
            shift: false,
            logo: false,
        };

        assert_eq!(
            default_tty_control_action("BackSpace", modifiers),
            Some(TtyControlAction::Quit)
        );
        assert_eq!(
            default_tty_control_action("Backspace", modifiers),
            Some(TtyControlAction::Quit)
        );
        assert_eq!(
            default_tty_control_action("F2", modifiers),
            Some(TtyControlAction::SwitchVt(2))
        );
        assert_eq!(
            default_tty_control_action("f3", modifiers),
            Some(TtyControlAction::SwitchVt(3))
        );
        assert_eq!(default_tty_control_action("F13", modifiers), None);
        assert_eq!(
            default_tty_control_action(
                "F3",
                ModifierSet {
                    ctrl: true,
                    alt: false,
                    shift: false,
                    logo: false,
                }
            ),
            None
        );
    }

    #[cfg(feature = "udev")]
    #[test]
    fn sync_primary_output_state_falls_back_when_no_outputs_exist() {
        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        let mut state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

        state.output_state.set_name("placeholder");
        state
            .output_state
            .set_logical_position(CanvasPoint::new(42.0, 24.0));
        state.sync_primary_output_state_from_space();

        assert_eq!(state.output_state.name(), "no-output");
        assert_eq!(
            state.output_state.logical_position(),
            CanvasPoint::new(0.0, 0.0)
        );
    }

    #[cfg(feature = "udev")]
    #[test]
    fn pointer_clamp_falls_back_to_viewport_without_outputs() {
        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        let state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

        let clamped = state.clamp_pointer_to_primary_output_or_viewport((-50.0, 9999.0).into());
        assert_eq!(clamped.x, 0.0);
        assert_eq!(clamped.y, 720.0);
    }

    #[test]
    fn resize_edges_from_maps_corner_edges() {
        let edges = resize_edges_from(xdg_toplevel::ResizeEdge::TopLeft);
        assert!(edges.left);
        assert!(edges.top);
        assert!(!edges.right);
        assert!(!edges.bottom);

        let edges = resize_edges_from(xdg_toplevel::ResizeEdge::BottomRight);
        assert!(edges.right);
        assert!(edges.bottom);
        assert!(!edges.left);
        assert!(!edges.top);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_hook_error_messages_include_hook_name_and_error() {
        let error = ConfigError::Validation("boom".into());
        assert_eq!(
            format_live_hook_error("key", &error),
            "live lua hook key failed: config validation error: boom"
        );
    }
}
