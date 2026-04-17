mod event_loop;
mod hooks;
mod input;
mod ipc;
mod protocols;
mod rendering;
mod window_management;
#[cfg(feature = "xwayland")]
mod xwayland;
#[cfg(feature = "udev")]
mod udev;

pub use crate::headless::{HeadlessOptions, HeadlessReport, HeadlessSession, run_headless};
#[cfg(feature = "udev")]
pub use udev::run_udev;
pub use event_loop::run_winit;

#[cfg(test)]
use event_loop::build_spawn_command;
use event_loop::spawn_client;
#[cfg(feature = "udev")]
use event_loop::publish_wayland_display;
#[cfg(any(test, feature = "udev"))]
use event_loop::startup_commands;
#[cfg(feature = "xwayland")]
use protocols::ClientState;
use protocols::{window_matches_surface, window_toplevel};
#[cfg(feature = "xwayland")]
use xwayland::spawn_xwayland;
#[cfg(feature = "lua")]
use hooks::format_live_hook_error;
#[cfg(test)]
use input::apply_trackpad_swipe_to_viewport;
#[cfg(test)]
use input::pinch_relative_factor;
#[cfg(test)]
use ipc::validate_ipc_screenshot_path;
#[cfg(feature = "udev")]
use rendering::{build_cursor_elements, build_live_space_elements};
use rendering::{
    format_keyspec, lock_overlay_elements, render_stack_front_to_back, resize_edges_from,
    solid_elements_from_draw_commands, write_ppm_screenshot,
};

#[cfg(feature = "udev")]
use std::{cell::RefCell, rc::Rc};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error::Error,
    ffi::OsString,
    io,
    os::unix::io::OwnedFd,
    sync::{Arc, atomic::{AtomicUsize, Ordering}},
    time::Duration,
};

use smithay::{
    backend::{
        input::{
            AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent,
            GesturePinchUpdateEvent, GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState,
            KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
        },
        renderer::{
            ExportMem, TextureMapping, damage::OutputDamageTracker, gles::GlesRenderer,
            utils::on_commit_buffer_handler,
        },
        winit::{self, WinitEvent},
    },
    desktop::{
        LayerSurface as DesktopLayerSurface, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords,
        layer_map_for_output,
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
            protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Logical, Point as SmithayPoint, Rectangle, SERIAL_COUNTER, Serial, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface, with_states,
        },
        idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState},
        output::{OutputHandler, OutputManagerState},
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
        },
        shell::{
            wlr_layer::{
                Layer as WlrLayer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler,
                WlrLayerShellState,
            },
            xdg::{
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
                XdgToplevelSurfaceData,
            },
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
    },
};

use crate::{
    canvas::{Point, Rect, Size, Viewport},
    input::{BindingMap, ModifierSet},
    lua::{
        ActionTarget, Config, ConfigError, OutputSnapshot, PointerSnapshot, RuntimeStateSnapshot,
        ViewportSnapshot, WindowSnapshot,
    },
    output::OutputState,
    output_management_protocol::{
        ModeInfo as OutputProtocolModeInfo, OutputHeadState as OutputProtocolHeadState,
        OutputManagementHandler as OutputProtocolHandler,
        OutputManagementState as OutputProtocolState,
        notify_changes as notify_output_management_changes,
    },
    window::{
        AppliedWindowRules, FocusStack, PlacementPolicy, ResizeEdges, Window as WindowModel,
        WindowId, WindowProperties, WindowRule,
    },
};

use crate::lua::{DrawCommand, DrawLayer, DrawSpace};
#[cfg(feature = "lua")]
use crate::lua::{LiveLuaHooks, ResolveFocusRequest};
#[cfg(feature = "udev")]
use smithay::{
    backend::renderer::element::{AsRenderElements, Wrap, utils::RescaleRenderElement},
    utils::Scale,
};
#[cfg(any(feature = "winit", feature = "x11", feature = "udev"))]
use smithay::{
    backend::renderer::{
        ImportAll,
        element::{Id, Kind, solid::SolidColorRenderElement},
    },
    utils::Physical,
};
#[cfg(feature = "xwayland")]
use smithay::{
    wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    xwayland::{
        X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent,
        xwm::{X11Window, XwmHandler, XwmId},
    },
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
    pub ipc_socket_path: std::path::PathBuf,
    pub display_handle: DisplayHandle,
    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    #[cfg(feature = "xwayland")]
    pub xwayland_shell_state: XWaylandShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub output_management_protocol_state: OutputProtocolState,
    pub seat_state: SeatState<EvilWm>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub idle_inhibit_state: IdleInhibitManagerState,
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
    remembered_app_sizes: HashMap<String, Size>,
    pending_client_default_size: HashSet<WindowId>,
    pub config: Option<Config>,
    pub config_path: Option<std::path::PathBuf>,
    redraw_requested: bool,
    trackpad_pinch_scale: Option<f64>,
    #[cfg(feature = "udev")]
    tty_control: Option<Rc<RefCell<Box<TtyControlCallback>>>>,
    #[cfg(feature = "udev")]
    tty_no_scanout_warned: bool,
    #[cfg(feature = "udev")]
    tty_session_active: bool,
    #[cfg(feature = "lua")]
    pub live_lua: Option<LiveLuaHooks>,
    #[cfg(feature = "lua")]
    live_hook_errors: BTreeMap<String, LiveHookErrorState>,
    active_interactive_op: Option<ActiveInteractiveOp>,
    last_pointer_button_pressed: Option<u32>,
    suppress_pointer_button_release: Option<u32>,
    warned_missing_move_update_hook: bool,
    warned_missing_resize_update_hook: bool,
    session_locked: bool,
    idle_inhibitors: HashSet<WlSurface>,
    pending_screenshot_path: Option<std::path::PathBuf>,
    event_log_path: Option<std::path::PathBuf>,
    ipc_trace_dir: Option<std::path::PathBuf>,
    event_sequence: u64,
    #[cfg(feature = "xwayland")]
    x11_wm: Option<X11Wm>,
    #[cfg(feature = "xwayland")]
    pending_x11_windows: HashSet<X11Window>,
}

impl Drop for EvilWm {
    fn drop(&mut self) {
        self.emit_event("shutdown", serde_json::json!({
            "backend": if cfg!(feature = "udev") && self.is_tty_backend() {
                "udev"
            } else {
                "winit"
            },
            "socket": self.socket_name.to_string_lossy(),
        }));
        if self.ipc_socket_path.exists() {
            let _ = std::fs::remove_file(&self.ipc_socket_path);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveInteractiveKind {
    Move,
    Resize,
    PanCanvas,
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

#[cfg(feature = "lua")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveHookErrorState {
    count: u64,
    last_error: String,
}

fn initialize_jsonl_file(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::File::create(path)?;
    Ok(())
}

fn append_jsonl(path: &std::path::Path, value: &serde_json::Value) -> io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn initialize_ipc_trace_dir(dir: &std::path::Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    initialize_jsonl_file(&dir.join("requests.jsonl"))?;
    initialize_jsonl_file(&dir.join("responses.jsonl"))?;
    Ok(())
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
            pid: None,
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
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        #[cfg(feature = "xwayland")]
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let popups = PopupManager::default();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let output_management_protocol_state = OutputProtocolState::new::<Self, _>(&dh, |_| true);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "evilwm");
        seat.add_keyboard(Default::default(), 200, 25)
            .map_err(|error| io::Error::other(format!("failed to add keyboard seat: {error}")))?;
        seat.add_pointer();
        let space = Space::default();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;
        let ipc_socket_path = Self::init_ipc_listener(event_loop)?;
        let loop_signal = event_loop.get_signal();

        let screen_size = Size::new(1280.0, 720.0);
        let mut viewport = Viewport::new(screen_size);
        let mut fallback_placement_policy = PlacementPolicy::default();
        if let Some(cfg) = &config {
            viewport = viewport
                .try_with_zoom_limits(cfg.canvas.min_zoom, cfg.canvas.max_zoom)
                .map_err(|error| io::Error::other(format!("invalid canvas zoom limits: {error}")))?;
            fallback_placement_policy.default_size = Size::new(900.0, 600.0);
        }

        let bindings = config
            .as_ref()
            .map(|cfg| {
                BindingMap::from_config(&cfg.bindings, cfg.canvas.pan_step, cfg.canvas.zoom_step)
            })
            .unwrap_or_default();
        let window_rules = compile_window_rules(config.as_ref());

        let event_log_path = std::env::var_os("EVILWM_EVENT_LOG").map(std::path::PathBuf::from);
        if let Some(path) = event_log_path.as_deref() {
            initialize_jsonl_file(path)?;
        }
        let ipc_trace_dir = std::env::var_os("EVILWM_IPC_TRACE_DIR").map(std::path::PathBuf::from);
        if let Some(dir) = ipc_trace_dir.as_deref() {
            initialize_ipc_trace_dir(dir)?;
        }

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
            .map_err(|error| {
                io::Error::other(format!("failed to initialize live lua hooks: {error}"))
            })?;

        Ok(Self {
            start_time,
            socket_name,
            ipc_socket_path,
            display_handle: dh,
            space,
            loop_signal,
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            #[cfg(feature = "xwayland")]
            xwayland_shell_state,
            shm_state,
            output_manager_state,
            output_management_protocol_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            idle_inhibit_state,
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
            remembered_app_sizes: HashMap::new(),
            pending_client_default_size: HashSet::new(),
            config,
            config_path,
            redraw_requested: true,
            trackpad_pinch_scale: None,
            #[cfg(feature = "udev")]
            tty_control: None,
            #[cfg(feature = "udev")]
            tty_no_scanout_warned: false,
            #[cfg(feature = "udev")]
            tty_session_active: true,
            #[cfg(feature = "lua")]
            live_lua,
            #[cfg(feature = "lua")]
            live_hook_errors: BTreeMap::new(),
            active_interactive_op: None,
            last_pointer_button_pressed: None,
            suppress_pointer_button_release: None,
            warned_missing_move_update_hook: false,
            warned_missing_resize_update_hook: false,
            session_locked: false,
            idle_inhibitors: HashSet::new(),
            pending_screenshot_path: None,
            event_log_path,
            ipc_trace_dir,
            event_sequence: 0,
            #[cfg(feature = "xwayland")]
            x11_wm: None,
            #[cfg(feature = "xwayland")]
            pending_x11_windows: HashSet::new(),
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

    pub(crate) fn emit_event(&mut self, kind: &str, data: serde_json::Value) {
        let Some(path) = self.event_log_path.clone() else {
            return;
        };

        self.event_sequence = self.event_sequence.saturating_add(1);
        let entry = serde_json::json!({
            "seq": self.event_sequence,
            "elapsed_ms": self.start_time.elapsed().as_millis() as u64,
            "kind": kind,
            "data": data,
        });
        if let Err(error) = append_jsonl(&path, &entry) {
            eprintln!("failed to append event log {}: {error}", path.display());
        }
    }

    pub(crate) fn trace_ipc_json(&mut self, file_name: &str, payload: serde_json::Value) {
        let Some(dir) = self.ipc_trace_dir.clone() else {
            return;
        };
        let path = dir.join(file_name);
        if let Err(error) = append_jsonl(&path, &payload) {
            eprintln!("failed to append ipc trace {}: {error}", path.display());
        }
    }

    fn init_wayland_listener(
        display: Display<EvilWm>,
        event_loop: &mut EventLoop<Self>,
    ) -> Result<OsString, Box<dyn Error>> {
        let listening_socket = ListeningSocketSource::new_auto().map_err(|error| {
            io::Error::other(format!("failed to create wayland socket: {error}"))
        })?;
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
            .map_err(|error| {
                io::Error::other(format!("failed to init wayland listener: {error}"))
            })?;

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
            .map_err(|error| {
                io::Error::other(format!(
                    "failed to register wayland dispatch source: {error}"
                ))
            })?;

        Ok(socket_name)
    }

    pub fn surface_under(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<(WlSurface, SmithayPoint<f64, Logical>)> {
        if !self.is_tty_backend()
            && let Some(output) = self.output_at_screen_position(pos)
            && let Some(output_geo) = self.space.output_geometry(&output)
        {
            let local = pos - output_geo.loc.to_f64();
            let map = layer_map_for_output(&output);
            for layer in [
                WlrLayer::Overlay,
                WlrLayer::Top,
                WlrLayer::Bottom,
                WlrLayer::Background,
            ] {
                if let Some(layer_surface) = map.layer_under(layer, local)
                    && let Some(layer_geo) = map.layer_geometry(layer_surface)
                    && let Some((surface, point)) = layer_surface
                        .surface_under(local - layer_geo.loc.to_f64(), WindowSurfaceType::ALL)
                {
                    return Some((surface, (point + layer_geo.loc + output_geo.loc).to_f64()));
                }
            }
        }

        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, point)| (surface, (point + location).to_f64()))
            })
    }

    fn output_layout_geometry(&self) -> Option<Rectangle<i32, Logical>> {
        let mut outputs = self.space.outputs();
        let first = outputs.next()?;
        let first_geo = self.space.output_geometry(first)?;

        let mut min_x = first_geo.loc.x;
        let mut min_y = first_geo.loc.y;
        let mut max_x = first_geo.loc.x + first_geo.size.w;
        let mut max_y = first_geo.loc.y + first_geo.size.h;

        for output in outputs {
            let Some(geo) = self.space.output_geometry(output) else {
                continue;
            };
            min_x = min_x.min(geo.loc.x);
            min_y = min_y.min(geo.loc.y);
            max_x = max_x.max(geo.loc.x + geo.size.w);
            max_y = max_y.max(geo.loc.y + geo.size.h);
        }

        Some(Rectangle::new(
            (min_x, min_y).into(),
            ((max_x - min_x), (max_y - min_y)).into(),
        ))
    }

    #[cfg(feature = "udev")]
    pub(crate) fn is_tty_backend(&self) -> bool {
        self.tty_control.is_some()
    }

    #[cfg(not(feature = "udev"))]
    pub(crate) fn is_tty_backend(&self) -> bool {
        false
    }

    fn output_at_screen_position(&self, pos: SmithayPoint<f64, Logical>) -> Option<Output> {
        self.space.outputs().find_map(|output| {
            let geometry = self.space.output_geometry(output)?;
            let within_x =
                pos.x >= geometry.loc.x as f64 && pos.x < (geometry.loc.x + geometry.size.w) as f64;
            let within_y =
                pos.y >= geometry.loc.y as f64 && pos.y < (geometry.loc.y + geometry.size.h) as f64;
            (within_x && within_y).then(|| output.clone())
        })
    }

    fn output_at_world_position(&self, pos: Point) -> Option<Output> {
        self.space.outputs().find_map(|output| {
            let output_state = self.output_state_for_output(output)?;
            let visible = output_state.viewport().visible_world_rect();
            let within_x = pos.x >= visible.origin.x && pos.x < visible.origin.x + visible.size.w;
            let within_y = pos.y >= visible.origin.y && pos.y < visible.origin.y + visible.size.h;
            (within_x && within_y).then(|| output.clone())
        })
    }

    fn screen_to_world_pointer_position(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> SmithayPoint<f64, Logical> {
        if !self.is_tty_backend() {
            return pos;
        }

        let Some(output) = self.output_at_screen_position(pos) else {
            return pos;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return pos;
        };
        let Some(output_state) = self.output_state_for_output(&output) else {
            return pos;
        };

        let local = Point::new(
            pos.x - output_geo.loc.x as f64,
            pos.y - output_geo.loc.y as f64,
        );
        let world = output_state.viewport().screen_to_world(local);
        (world.x, world.y).into()
    }

    fn clamp_pointer_to_output_layout_or_viewport(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> SmithayPoint<f64, Logical> {
        if self.is_tty_backend() {
            let mut outputs = self.space.outputs();
            let Some(first) = outputs.next() else {
                let fallback = self.viewport().visible_world_rect();
                return (
                    pos.x.clamp(
                        fallback.origin.x,
                        fallback.origin.x + fallback.size.w.max(1.0),
                    ),
                    pos.y.clamp(
                        fallback.origin.y,
                        fallback.origin.y + fallback.size.h.max(1.0),
                    ),
                )
                    .into();
            };
            let Some(first_state) = self.output_state_for_output(first) else {
                return pos;
            };
            let first_visible = first_state.viewport().visible_world_rect();
            let mut min_x = first_visible.origin.x;
            let mut min_y = first_visible.origin.y;
            let mut max_x = first_visible.origin.x + first_visible.size.w;
            let mut max_y = first_visible.origin.y + first_visible.size.h;

            for output in outputs {
                let Some(output_state) = self.output_state_for_output(output) else {
                    continue;
                };
                let visible = output_state.viewport().visible_world_rect();
                min_x = min_x.min(visible.origin.x);
                min_y = min_y.min(visible.origin.y);
                max_x = max_x.max(visible.origin.x + visible.size.w);
                max_y = max_y.max(visible.origin.y + visible.size.h);
            }

            return (pos.x.clamp(min_x, max_x), pos.y.clamp(min_y, max_y)).into();
        }

        if let Some(output_geo) = self.output_layout_geometry() {
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
        if let Some(output_geo) = self.output_layout_geometry() {
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
                if self.is_tty_backend() {
                    let world = Point::new(pointer.x, pointer.y);
                    if let Some(output) = self.output_at_world_position(world)
                        && let Some(output_state) = self.output_state_for_output(&output)
                    {
                        return output_state.viewport().world_to_screen(world);
                    }
                    return Point::new(pointer.x, pointer.y);
                }

                let logical = self.output_state.logical_position();
                Point::new(pointer.x - logical.x, pointer.y - logical.y)
            });

        let screen = self.viewport().screen_size();
        let fallback = Point::new(screen.w / 2.0, screen.h / 2.0);
        let pointer = local_pointer.unwrap_or(fallback);
        Point::new(
            pointer.x.clamp(0.0, screen.w),
            pointer.y.clamp(0.0, screen.h),
        )
    }

    fn hovered_window_snapshot_at(
        &self,
        pos: SmithayPoint<f64, Logical>,
    ) -> Option<WindowSnapshot> {
        self.surface_under(pos)
            .and_then(|(surface, _)| self.window_id_for_surface(&surface))
            .and_then(|id| self.window_snapshot_for_id(id))
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

    fn sync_output_state(&mut self, name: &str, logical_position: Point, screen_size: Size) {
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

    fn notify_output_management_state(&mut self) {
        let mut heads = HashMap::new();
        for output in self.space.outputs() {
            let Some(mode) = output.current_mode() else {
                continue;
            };
            let Some(geometry) = self.space.output_geometry(output) else {
                continue;
            };
            let physical = output.physical_properties();
            heads.insert(
                output.name(),
                OutputProtocolHeadState {
                    name: output.name(),
                    description: format!("{} {}", physical.make, physical.model),
                    make: physical.make,
                    model: physical.model,
                    serial_number: String::new(),
                    physical_size: (physical.size.w, physical.size.h),
                    modes: vec![OutputProtocolModeInfo {
                        width: mode.size.w,
                        height: mode.size.h,
                        refresh: mode.refresh,
                        preferred: true,
                    }],
                    current_mode_index: Some(0),
                    position: (geometry.loc.x, geometry.loc.y),
                    transform: output.current_transform(),
                    scale: output.current_scale().fractional_scale(),
                },
            );
        }

        notify_output_management_changes::<Self>(&mut self.output_management_protocol_state, heads);
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

    fn zoom_all_viewports_at_primary(&mut self, anchor: Point, factor: f64) {
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
                    let output_id = self.output_id_for_window_bounds(window.bounds);
                    WindowSnapshot {
                        id: window.id.0,
                        app_id: window.properties.app_id.clone(),
                        title: window.properties.title.clone(),
                        bounds: window.bounds,
                        floating: window.floating,
                        exclude_from_focus: window.exclude_from_focus,
                        focused: focused == Some(window.id),
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

#[cfg(test)]
mod tests {
    use super::{
        ActiveInteractiveKind, ActiveInteractiveOp, apply_trackpad_swipe_to_viewport,
        build_spawn_command, compile_window_rules, pinch_relative_factor,
        render_stack_front_to_back, resize_edges_from,
    };
    #[cfg(feature = "udev")]
    use super::{TtyControlAction, default_tty_control_action};
    #[cfg(feature = "lua")]
    use super::{format_live_hook_error, startup_commands};
    #[cfg(any(feature = "lua", feature = "udev"))]
    use crate::canvas::Point as CanvasPoint;
    use crate::canvas::{Size, Viewport};
    #[cfg(feature = "lua")]
    use crate::compositor::EvilWm;
    #[cfg(feature = "lua")]
    use crate::input::{BindingMap, ModifierSet};
    #[cfg(feature = "lua")]
    use crate::lua::{
        BindingConfig, CanvasConfig, Config, ConfigError, DrawConfig, DrawLayer, LiveLuaHooks,
        ResolveFocusRequest,
    };
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
    #[test]
    fn remembered_size_wins_over_client_preferred_size() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: DrawConfig::default(),
            window: crate::lua::WindowConfig {
                use_client_default_size: true,
                remember_sizes_by_app_id: true,
            },
            autostart: Vec::new(),
            bindings: Vec::new(),
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        };
        let mut state = create_live_test_state(Some(config));
        state
            .remembered_app_sizes
            .insert("xterm".into(), Size::new(900.0, 700.0));

        let properties = crate::window::WindowProperties {
            app_id: Some("xterm".into()),
            title: Some("xterm".into()),
            pid: None,
        };
        let chosen = state.requested_initial_window_size_from(&properties, Some(Size::new(640.0, 480.0)));
        assert_eq!(chosen, Some(Size::new(900.0, 700.0)));
    }

    #[cfg(feature = "lua")]
    #[test]
    fn client_preferred_size_is_used_when_no_remembered_size_exists() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: DrawConfig::default(),
            window: crate::lua::WindowConfig {
                use_client_default_size: true,
                remember_sizes_by_app_id: true,
            },
            autostart: Vec::new(),
            bindings: Vec::new(),
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        };
        let state = create_live_test_state(Some(config));
        let properties = crate::window::WindowProperties {
            app_id: Some("xterm".into()),
            title: Some("xterm".into()),
            pid: None,
        };
        let chosen = state.requested_initial_window_size_from(&properties, Some(Size::new(640.0, 480.0)));
        assert_eq!(chosen, Some(Size::new(640.0, 480.0)));
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
        state.register_output_state(
            "right",
            CanvasPoint::new(1280.0, 0.0),
            Size::new(1920.0, 1080.0),
        );

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
        state.register_output_state(
            "right",
            CanvasPoint::new(800.0, 0.0),
            Size::new(1600.0, 900.0),
        );

        {
            let left_state = state
                .output_state_for_name_mut("left")
                .expect("left output state");
            left_state
                .viewport_mut()
                .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
        }
        {
            let right_state = state
                .output_state_for_name_mut("right")
                .expect("right output state");
            right_state
                .viewport_mut()
                .pan_world(crate::canvas::Vec2::new(-20.0, 10.0));
            right_state
                .viewport_mut()
                .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);
        }

        let snapshot = state.state_snapshot();
        let left_output = snapshot
            .outputs
            .iter()
            .find(|output| output.id == "left")
            .expect("left snapshot");
        let right_output = snapshot
            .outputs
            .iter()
            .find(|output| output.id == "right")
            .expect("right snapshot");

        assert_eq!(
            left_output.viewport.visible_world,
            crate::canvas::Rect::new(100.0, 50.0, 800.0, 600.0)
        );
        assert_eq!(
            right_output.viewport.visible_world,
            crate::canvas::Rect::new(-20.0, 10.0, 800.0, 450.0)
        );
        assert_ne!(
            left_output.viewport.visible_world,
            right_output.viewport.visible_world
        );
    }

    #[cfg(all(feature = "lua", feature = "udev"))]
    #[test]
    fn tty_screen_pointer_positions_map_into_world_space() {
        let mut state = create_live_test_state(None);
        state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

        let output = create_test_output("tty", (0, 0), (800, 600));
        state.space.map_output(&output, (0, 0));
        state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        let output_state = state
            .output_state_for_name_mut("tty")
            .expect("output state");
        output_state
            .viewport_mut()
            .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
        output_state
            .viewport_mut()
            .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);

        let mapped = state.screen_to_world_pointer_position((200.0, 100.0).into());
        assert_eq!(mapped, (200.0, 100.0).into());
    }

    #[cfg(all(feature = "lua", feature = "udev"))]
    #[test]
    fn tty_cursor_uses_world_pointer_position() {
        let mut state = create_live_test_state(None);
        state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

        let output = create_test_output("tty", (0, 0), (800, 600));
        state.space.map_output(&output, (0, 0));
        state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        let output_state = state
            .output_state_for_name_mut("tty")
            .expect("output state");
        output_state
            .viewport_mut()
            .pan_world(crate::canvas::Vec2::new(100.0, 50.0));
        output_state
            .viewport_mut()
            .zoom_at_screen(CanvasPoint::new(0.0, 0.0), 2.0);

        state
            .seat
            .get_pointer()
            .expect("pointer")
            .set_location((200.0, 100.0).into());
        let elements = super::build_cursor_elements(&state, &output);
        assert!(!elements.is_empty());
    }

    #[cfg(all(feature = "lua", feature = "udev"))]
    #[test]
    fn tty_pan_keeps_cursor_screen_position_stable() {
        let mut state = create_live_test_state(None);
        state.tty_control = Some(std::rc::Rc::new(std::cell::RefCell::new(Box::new(|_| {}))));

        let output = create_test_output("tty", (0, 0), (800, 600));
        state.space.map_output(&output, (0, 0));
        state.register_output_state("tty", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        state
            .seat
            .get_pointer()
            .expect("pointer")
            .set_location((200.0, 100.0).into());

        let before_world = state
            .seat
            .get_pointer()
            .expect("pointer")
            .current_location();
        let before_screen = state
            .output_state_for_output(&output)
            .expect("output state")
            .viewport()
            .world_to_screen(CanvasPoint::new(before_world.x, before_world.y));

        state.pan_all_viewports(crate::canvas::Vec2::new(30.0, -20.0));

        let after_world = state
            .seat
            .get_pointer()
            .expect("pointer")
            .current_location();
        let after_screen = state
            .output_state_for_output(&output)
            .expect("output state")
            .viewport()
            .world_to_screen(CanvasPoint::new(after_world.x, after_world.y));

        assert_eq!(after_world, (230.0, 80.0).into());
        assert_eq!(before_screen, after_screen);
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
        let cmd = build_spawn_command(
            "foot --server",
            OsStr::new("wayland-99"),
            std::path::Path::new("/tmp/evilwm-ipc.sock"),
        )
        .expect("spawn command");
        let envs = cmd.get_envs().collect::<Vec<_>>();
        assert!(envs.iter().any(|(key, value)| {
            key == &OsStr::new("WAYLAND_DISPLAY") && value == &Some(OsStr::new("wayland-99"))
        }));
        assert!(envs.iter().any(|(key, value)| {
            key == &OsStr::new("EVILWM_IPC_SOCKET")
                && value == &Some(OsStr::new("/tmp/evilwm-ipc.sock"))
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
            std::path::Path::new("/tmp/evilwm-ipc.sock"),
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
        assert!(
            build_spawn_command(
                "   ",
                OsStr::new("wayland-99"),
                std::path::Path::new("/tmp/evilwm-ipc.sock"),
            )
            .is_none()
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn compile_window_rules_preserves_matchers_and_actions() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: crate::lua::DrawConfig::default(),
            window: crate::lua::WindowConfig::default(),
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
    fn lua_configured_draw_stack_controls_render_order() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: DrawConfig {
                stack: vec![
                    DrawLayer::Background,
                    DrawLayer::Cursor,
                    DrawLayer::Windows,
                    DrawLayer::Overlay,
                ],
            },
            window: crate::lua::WindowConfig::default(),
            autostart: Vec::new(),
            bindings: Vec::new(),
            rules: Vec::new(),
            source_root: std::path::PathBuf::from("."),
        };
        let state = create_live_test_state(Some(config));

        assert_eq!(
            render_stack_front_to_back(&state),
            vec![
                DrawLayer::Overlay,
                DrawLayer::Windows,
                DrawLayer::Cursor,
                DrawLayer::Background,
            ]
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn resolve_focus_can_start_modifier_drag() {
        let mut state = create_live_test_state(None);
        let id = WindowId(3);
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(100.0, 120.0, 300.0, 200.0)),
        );
        state.last_pointer_button_pressed = Some(272);
        state
            .seat
            .get_pointer()
            .expect("pointer")
            .set_location((160.0, 170.0).into());

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.resolve_focus = function(ctx)
                  if ctx.reason == "pointer_button" and ctx.window and ctx.pressed and ctx.modifiers and ctx.modifiers.super and ctx.button == 272 then
                    return {
                      actions = {
                        { kind = "focus_window", id = ctx.window.id },
                        { kind = "begin_move", id = ctx.window.id },
                      },
                    }
                  end
                end
                "#,
                "resolve-drag.lua",
            )
            .expect("load hooks");
        state.live_lua = Some(hooks);

        let snapshot = state.window_snapshot_for_id(id);
        assert!(state.try_live_resolve_focus(ResolveFocusRequest {
            reason: "pointer_button",
            window: snapshot.as_ref(),
            previous: None,
            pointer: Some(CanvasPoint::new(160.0, 170.0)),
            button: Some(272),
            pressed: Some(true),
            modifiers: Some(ModifierSet {
                ctrl: false,
                alt: false,
                shift: false,
                logo: true
            }),
        }));
        assert_eq!(state.focus_stack.focused(), Some(id));
        assert!(matches!(
            state.active_interactive_op.as_ref().map(|op| op.kind),
            Some(ActiveInteractiveKind::Move)
        ));
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_key_hook_runs_before_rust_binding_fallback() {
        let config = Config {
            backend: Some("winit".into()),
            canvas: CanvasConfig::default(),
            draw: crate::lua::DrawConfig::default(),
            window: crate::lua::WindowConfig::default(),
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
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(100.0, 120.0, 300.0, 200.0)),
        );
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
            draw: crate::lua::DrawConfig::default(),
            window: crate::lua::WindowConfig::default(),
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
        assert_eq!(
            state.viewport().world_origin(),
            CanvasPoint::new(-32.0, 0.0)
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn active_interactive_move_helper_advances_and_finishes_sequence() {
        let mut state = create_live_test_state(None);
        let id = WindowId(6);
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
        );

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.move_update = function(ctx)
                  evil.window.move(ctx.window.id, 11, 22)
                end
                evil.on.move_end = function(ctx)
                  evil.window.move(ctx.window.id, 33, 44)
                end
                "#,
                "live-move-helper.lua",
            )
            .expect("load move helper hooks");
        state.live_lua = Some(hooks);
        state.active_interactive_op = Some(ActiveInteractiveOp::new(
            ActiveInteractiveKind::Move,
            id,
            CanvasPoint::new(10.0, 10.0),
            None,
            Some(274),
        ));

        state.advance_active_interactive_op(CanvasPoint::new(20.0, 25.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(11.0, 22.0)
        );

        state.finish_active_interactive_op(272, CanvasPoint::new(20.0, 25.0));
        assert!(
            state.active_interactive_op.is_some(),
            "wrong button must not end move"
        );

        state.finish_active_interactive_op(274, CanvasPoint::new(30.0, 35.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(33.0, 44.0)
        );
        assert!(
            state.active_interactive_op.is_none(),
            "correct button must end move"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_move_hook_sequence_updates_window_model() {
        let mut state = create_live_test_state(None);
        let id = WindowId(7);
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
        );

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
        state.trigger_live_move_update(
            id,
            crate::canvas::Vec2::new(4.0, 6.0),
            CanvasPoint::new(4.0, 6.0),
        );
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(30.0, 40.0)
        );
        state.trigger_live_move_end(
            id,
            crate::canvas::Vec2::new(9.0, 12.0),
            CanvasPoint::new(9.0, 12.0),
        );
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds.origin,
            CanvasPoint::new(50.0, 60.0)
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn active_interactive_resize_helper_advances_and_finishes_sequence() {
        let mut state = create_live_test_state(None);
        let id = WindowId(8);
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
        );

        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.resize_update = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 1, 2, 350, 225)
                end
                evil.on.resize_end = function(ctx)
                  evil.window.set_bounds(ctx.window.id, 3, 4, 400, 260)
                end
                "#,
                "live-resize-helper.lua",
            )
            .expect("load resize helper hooks");
        state.live_lua = Some(hooks);
        let edges = crate::window::ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        state.active_interactive_op = Some(ActiveInteractiveOp::new(
            ActiveInteractiveKind::Resize,
            id,
            CanvasPoint::new(10.0, 10.0),
            Some(edges),
            Some(274),
        ));

        state.advance_active_interactive_op(CanvasPoint::new(25.0, 30.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds,
            crate::canvas::Rect::new(1.0, 2.0, 350.0, 225.0)
        );

        state.finish_active_interactive_op(274, CanvasPoint::new(30.0, 35.0));
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds,
            crate::canvas::Rect::new(3.0, 4.0, 400.0, 260.0)
        );
        assert!(
            state.active_interactive_op.is_none(),
            "correct button must end resize"
        );
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_resize_hook_sequence_updates_window_model() {
        let mut state = create_live_test_state(None);
        let id = WindowId(9);
        state.window_models.insert(
            id,
            Window::new(id, crate::canvas::Rect::new(0.0, 0.0, 300.0, 200.0)),
        );

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
        state.trigger_live_resize_update(
            id,
            crate::canvas::Vec2::new(6.0, 9.0),
            CanvasPoint::new(6.0, 9.0),
            edges,
        );
        assert_eq!(
            state.window_models.get(&id).expect("window").bounds,
            crate::canvas::Rect::new(5.0, 6.0, 360.0, 240.0)
        );
        state.trigger_live_resize_end(
            id,
            crate::canvas::Vec2::new(8.0, 10.0),
            CanvasPoint::new(8.0, 10.0),
            edges,
        );
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
                draw: crate::lua::DrawConfig::default(),
                window: crate::lua::WindowConfig::default(),
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

        let clamped = state.clamp_pointer_to_output_layout_or_viewport((-50.0, 9999.0).into());
        assert_eq!(clamped.x, 0.0);
        assert_eq!(clamped.y, 720.0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn pointer_clamp_uses_full_output_layout_bounds() {
        let mut state = create_live_test_state(None);
        let left = create_test_output("left", (0, 0), (800, 600));
        let right = create_test_output("right", (800, 0), (1024, 768));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (800, 0));
        state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        state.register_output_state(
            "right",
            CanvasPoint::new(800.0, 0.0),
            Size::new(1024.0, 768.0),
        );

        let clamped = state.clamp_pointer_to_output_layout_or_viewport((2500.0, -50.0).into());
        assert_eq!(clamped.x, 1824.0);
        assert_eq!(clamped.y, 0.0);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn window_output_association_uses_center_point_inside_visible_world() {
        let mut state = create_live_test_state(None);
        let left = create_test_output("left", (0, 0), (800, 600));
        let right = create_test_output("right", (800, 0), (800, 600));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (800, 0));
        state.register_output_state("left", CanvasPoint::new(0.0, 0.0), Size::new(800.0, 600.0));
        state.register_output_state(
            "right",
            CanvasPoint::new(800.0, 0.0),
            Size::new(800.0, 600.0),
        );
        state
            .output_state_for_name_mut("right")
            .expect("right output state")
            .viewport_mut()
            .pan_world(crate::canvas::Vec2::new(800.0, 0.0));

        let left_id =
            state.output_id_for_window_bounds(crate::canvas::Rect::new(100.0, 100.0, 200.0, 200.0));
        let right_id = state
            .output_id_for_window_bounds(crate::canvas::Rect::new(1000.0, 100.0, 200.0, 200.0));

        assert_eq!(left_id.as_deref(), Some("left"));
        assert_eq!(right_id.as_deref(), Some("right"));
    }

    #[cfg(feature = "udev")]
    #[test]
    fn sync_primary_output_state_uses_remaining_output_after_primary_removal() {
        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        let mut state = EvilWm::new(&mut event_loop, display, None, None).expect("state");
        let left = create_test_output("left", (0, 0), (800, 600));
        let right = create_test_output("right", (800, 0), (1024, 768));
        state.space.map_output(&left, (0, 0));
        state.space.map_output(&right, (800, 0));
        state.sync_primary_output_state_from_space();
        assert_eq!(state.output_state.name(), "left");

        state.space.unmap_output(&left);
        state.sync_primary_output_state_from_space();
        assert_eq!(state.output_state.name(), "right");
        assert_eq!(
            state.output_state.logical_position(),
            CanvasPoint::new(800.0, 0.0)
        );
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

    #[test]
    fn validate_ipc_screenshot_path_allows_tmp_and_rejects_outside_roots() {
        let tmp_path = std::env::temp_dir().join("evilwm-test-capture.ppm");
        let validated = super::validate_ipc_screenshot_path(&tmp_path).expect("tmp screenshot path");
        assert!(validated.starts_with(std::env::temp_dir()));

        let rejected = super::validate_ipc_screenshot_path(std::path::Path::new("/etc/evilwm.ppm"));
        assert!(rejected.is_err());
    }

    #[test]
    fn ipc_socket_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let mut event_loop: EventLoop<EvilWm> = EventLoop::try_new().expect("event loop");
        let display: Display<EvilWm> = Display::new().expect("display");
        let state = EvilWm::new(&mut event_loop, display, None, None).expect("state");

        let mode = std::fs::metadata(&state.ipc_socket_path)
            .expect("ipc socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_hook_errors_are_captured_in_runtime_snapshot() {
        let mut state = create_live_test_state(None);
        let hooks = LiveLuaHooks::new(".").expect("live hooks");
        hooks
            .load_script_str(
                r#"
                evil.on.key = function(ctx)
                  error("boom")
                end
                "#,
                "hook-error.lua",
            )
            .expect("load hooks");
        state.live_lua = Some(hooks);

        assert!(!state.trigger_live_key("Super+H".to_string()));

        let snapshot = crate::ipc::RuntimeSnapshot::from_live(&state);
        assert_eq!(snapshot.hook_errors.len(), 1);
        assert_eq!(snapshot.hook_errors[0].hook, "key");
        assert_eq!(snapshot.hook_errors[0].count, 1);
        assert!(snapshot.hook_errors[0].last_error.contains("evil.on.key"));
    }

    #[cfg(feature = "lua")]
    #[test]
    fn live_hook_error_messages_include_hook_name_and_error() {
        let error = ConfigError::Validation("boom".into());
        assert_eq!(
            format_live_hook_error("key", &error),
            "[evilwm] lua hook error: evil.on.key — config validation error: boom"
        );
    }
}
